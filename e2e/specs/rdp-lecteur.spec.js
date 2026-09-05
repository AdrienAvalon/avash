// Lecteur partagé (redirection de lecteur RDPDR) : un dossier du poste, donné
// dans le formulaire de connexion directe, est servi au serveur de test, qui
// joue son scénario (annonce, volume, énumération, lecture de bonjour.txt,
// écriture d'ecrit.txt) et raconte chaque étape sur sa sortie standard.
//
// Deux chemins : l'interface entière (formulaire → rdp_open → sidecar), puis
// le sidecar seul avec le son coupé, car MS-RDPEFS exige que rdpdr soit
// annoncé avec rdpsnd : sans le canal audio muet, le lecteur ne répondrait
// plus dès que l'utilisateur coupe le son.
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { startRdpServer, waitForPort, attendreBureauConnecte } from "./helpers.js";

const PORT = 33896;
const CONTENU = "Bonjour depuis le poste.\nDeuxième ligne, avec des accents : é à ü.\n";

/** Un dossier partagé neuf, avec bonjour.txt dedans. */
function dossierPartage() {
  const d = mkdtempSync(join(tmpdir(), "avash-e2e-lecteur-"));
  writeFileSync(join(d, "bonjour.txt"), CONTENU);
  writeFileSync(join(d, "autre.bin"), Buffer.from([1, 2, 3]));
  return d;
}

/** Attend que les lignes du serveur contiennent `attendu`. */
async function attendreLigne(lignes, attendu, timeout = 20000) {
  await browser.waitUntil(() => lignes.some((l) => l.includes(attendu)), {
    timeout,
    timeoutMsg: `le serveur n'a pas dit « ${attendu} » ; il a dit :\n${lignes.join("\n")}`,
  });
}

/** Ce que le scénario du serveur doit avoir vu et fait dans `dossier`. */
async function verifierScenario(lignes, dossier) {
  await attendreLigne(lignes, "rdpdr: lecteur Avash annoncé");
  await attendreLigne(lignes, "rdpdr: volume AVASH");
  await attendreLigne(lignes, "rdpdr: entrée bonjour.txt");
  await attendreLigne(lignes, "rdpdr: entrée autre.bin");
  const sha = createHash("sha256").update(CONTENU).digest("hex");
  await attendreLigne(lignes, `rdpdr: lu bonjour.txt ${Buffer.byteLength(CONTENU)} octets sha256=${sha}`);
  await attendreLigne(lignes, "rdpdr: écrit ecrit.txt");
  await attendreLigne(lignes, "rdpdr: scénario terminé");
  expect(lignes.some((l) => l.includes("rdpdr: échec"))).toBe(false);
  expect(readFileSync(join(dossier, "ecrit.txt"), "utf8")).toBe("depuis le serveur\n");
  expect(readFileSync(join(dossier, "bonjour.txt"), "utf8")).toBe(CONTENU);
}

describe("RDP — lecteur partagé (redirection de lecteur)", () => {
  let srv;
  let lignes;
  let dossier;
  before(async () => {
    lignes = [];
    srv = startRdpServer(PORT, (d) => lignes.push(...d.split("\n").filter(Boolean)));
    await waitForPort(PORT);
  });
  after(() => {
    if (srv) srv.kill();
    if (dossier) rmSync(dossier, { recursive: true, force: true });
  });

  it("le dossier du formulaire est servi au distant, qui le lit et y écrit", async () => {
    dossier = dossierPartage();
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => {
      const r = document.querySelector('input[name="proto"][value="rdp"]');
      r.checked = true;
      r.dispatchEvent(new Event("change", { bubbles: true }));
    });
    // Le champ du dossier partagé n'apparaît qu'en RDP.
    expect(await $("#m-rdp-partage-row").isDisplayed()).toBe(true);
    await $("#m-addr").setValue("127.0.0.1");
    await $("#m-port").setValue(String(PORT));
    await $("#m-user").setValue("test");
    await $("#m-password").setValue("test");
    await browser.execute((d) => {
      const champ = document.getElementById("m-rdp-partage");
      champ.value = d;
      champ.dispatchEvent(new Event("input", { bubbles: true }));
    }, dossier);
    await $("#m-submit").click();

    await $(".rdp-container canvas").waitForExist({ timeout: 20000, timeoutMsg: "aucun canvas RDP" });
    await attendreBureauConnecte();
    await verifierScenario(lignes, dossier);
  });

  it("le son coupé, le lecteur répond toujours (canal audio muet)", async () => {
    // Le serveur de test sert ses clients l'un après l'autre : le bureau du
    // premier scénario doit être fermé avant qu'un second client se présente,
    // sans quoi, sur une machine chargée, le sidecar attend son tour au-delà
    // du délai (vu en suite complète, 35 fichiers en parallèle).
    await browser.execute(() => document.querySelector(".tab.active .close")?.click());
    await browser.waitUntil(async () => (await $$(".rdp-container")).length === 0, { timeout: 5000 });
    lignes.length = 0;
    rmSync(dossier, { recursive: true, force: true });
    dossier = dossierPartage();
    const sidecar = spawn(
      resolve("../rdp-sidecar/target/release/avash-rdp"),
      ["--host", "127.0.0.1", "--port", String(PORT), "-u", "test", "--width", "800", "--height", "600",
       "--sans-son", "--lecteur", dossier],
      { stdio: ["pipe", "pipe", "ignore"] },
    );
    sidecar.stdin.write("test\n");
    try {
      const [wsPort, token] = await new Promise((ok, ko) => {
        const t = setTimeout(() => ko(new Error("le sidecar n'a rien annoncé")), 30000);
        sidecar.stdout.once("data", (b) => { clearTimeout(t); ok(b.toString().trim().split(/\s+/)); });
      });
      const ws = new WebSocket(`ws://127.0.0.1:${wsPort}`);
      ws.binaryType = "arraybuffer";
      let blocsSon = 0;
      const connecte = new Promise((ok, ko) => {
        const t = setTimeout(() => ko(new Error("connexion RDP jamais annoncée")), 25000);
        ws.onmessage = (ev) => {
          const octets = new Uint8Array(ev.data);
          if (octets[0] === 1) { clearTimeout(t); ok(); }
          if (octets[0] === 20) blocsSon += 1;
        };
      });
      ws.onopen = () => ws.send(new TextEncoder().encode(token));
      await connecte;
      try {
        await verifierScenario(lignes, dossier);
        expect(blocsSon).toBe(0);
      } finally {
        ws.close();
      }
    } finally {
      sidecar.kill();
    }
  });
});
