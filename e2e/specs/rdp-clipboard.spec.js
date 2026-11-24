// Presse-papiers RDP, sens « bureau distant → poste ».
//
// Ce scénario ne passe PAS par l'interface : il pilote le sidecar `avash-rdp`
// directement sur son WebSocket. C'est volontaire — l'alternative (vérifier le
// presse-papiers du système) écraserait celui de l'utilisateur et dépendrait de
// l'environnement de bureau. Ici on valide la chaîne réelle :
//   serveur de test (CLIPRDR) → sidecar → trame [8] destinée au front.
import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { startRdpServer, waitForPort } from "./helpers.js";

const PORT = 33897;
const ATTENDU = "avash-cliprdr-test"; // cf. CLIP_TEXT du serveur de test
const MSG_CONNECTE = 1;
const MSG_PRESSE_PAPIERS = 8;

describe("RDP — presse-papiers du bureau distant vers le poste", () => {
  let srv;
  let sidecar;

  before(async () => {
    srv = startRdpServer(PORT);
    await waitForPort(PORT);
  });
  after(() => {
    if (sidecar) sidecar.kill();
    if (srv) srv.kill();
  });

  it("le texte copié côté serveur arrive au front", async () => {
    // 1) Lancer le sidecar (mot de passe par stdin, jamais en argument).
    sidecar = spawn(
      resolve("../rdp-sidecar/target/release/avash-rdp"),
      ["--host", "127.0.0.1", "--port", String(PORT), "-u", "test", "--width", "1024", "--height", "768"],
      { stdio: ["pipe", "pipe", "ignore"] },
    );
    sidecar.stdin.write("test\n");

    // 2) Il annonce « port jeton » sur sa sortie standard.
    const [wsPort, token] = await new Promise((ok, ko) => {
      const t = setTimeout(() => ko(new Error("le sidecar n'a rien annoncé")), 15000);
      sidecar.stdout.once("data", (b) => {
        clearTimeout(t);
        ok(b.toString().trim().split(/\s+/));
      });
    });

    // 3) Se connecter, présenter le jeton, puis attendre la trame [8].
    const ws = new WebSocket(`ws://127.0.0.1:${wsPort}`);
    ws.binaryType = "arraybuffer";
    const texte = await new Promise((ok, ko) => {
      const t = setTimeout(() => ko(new Error("aucune trame presse-papiers reçue")), 25000);
      let connecte = false;
      ws.onopen = () => ws.send(new TextEncoder().encode(token));
      ws.onerror = () => { clearTimeout(t); ko(new Error("WebSocket en erreur")); };
      ws.onmessage = (ev) => {
        const octets = new Uint8Array(ev.data);
        if (octets[0] === MSG_CONNECTE) connecte = true;
        if (octets[0] === MSG_PRESSE_PAPIERS) {
          clearTimeout(t);
          ok({ texte: new TextDecoder().decode(octets.slice(1)), connecte });
        }
      };
    });

    expect(texte.connecte).toBe(true);      // la session RDP s'est bien établie
    expect(texte.texte).toBe(ATTENDU);      // et le presse-papiers distant est arrivé
    ws.close();
  });
});

// Sens « poste → bureau distant », signalé sur Windows Server 2012 R2 le
// 24 novembre 2026 : un Ctrl+V collait l'ancien contenu. L'interface envoie le
// texte du poste [8] puis la touche [4] ; le processus servait le WebSocket en
// priorité et annonçait le texte au serveur APRÈS la touche. Le serveur de test
// dit sur sa sortie dans quel ordre il a reçu l'annonce et la frappe.
describe("RDP — l'annonce du presse-papiers du poste précède la touche qui colle", () => {
  const PORT_ORDRE = 33910;
  let srv;
  let sidecar;
  let lignes = [];

  before(async () => {
    srv = startRdpServer(PORT_ORDRE, (d) => lignes.push(...d.split("\n").filter(Boolean)));
    await waitForPort(PORT_ORDRE);
  });
  after(() => {
    if (sidecar) sidecar.kill();
    if (srv) srv.kill();
  });

  it("le serveur reçoit le texte annoncé avant la frappe", async () => {
    sidecar = spawn(
      resolve("../rdp-sidecar/target/release/avash-rdp"),
      ["--host", "127.0.0.1", "--port", String(PORT_ORDRE), "-u", "test", "--width", "1024", "--height", "768"],
      { stdio: ["pipe", "pipe", "ignore"] },
    );
    sidecar.stdin.write("test\n");
    const [wsPort, token] = await new Promise((ok, ko) => {
      const t = setTimeout(() => ko(new Error("le sidecar n'a rien annoncé")), 15000);
      sidecar.stdout.once("data", (b) => { clearTimeout(t); ok(b.toString().trim().split(/\s+/)); });
    });
    const ws = new WebSocket(`ws://127.0.0.1:${wsPort}`);
    ws.binaryType = "arraybuffer";
    await new Promise((ok, ko) => {
      const t = setTimeout(() => ko(new Error("session RDP jamais connectée")), 25000);
      ws.onopen = () => ws.send(new TextEncoder().encode(token));
      ws.onerror = () => { clearTimeout(t); ko(new Error("WebSocket en erreur")); };
      ws.onmessage = (ev) => { if (new Uint8Array(ev.data)[0] === MSG_CONNECTE) { clearTimeout(t); ok(); } };
    });
    // Le canal CLIPRDR se prépare juste après la connexion : on le laisse finir,
    // puis on oublie ce que le serveur a dit jusque-là (sa propre annonce).
    await new Promise((r) => setTimeout(r, 1500));
    lignes = [];
    ws.send(new Uint8Array([12, 1])); // partage du presse-papiers autorisé
    const texte = new TextEncoder().encode("texte copié sur le poste");
    const annonce = new Uint8Array(1 + texte.length);
    annonce[0] = 8;
    annonce.set(texte, 1);
    // Exactement ce que fait l'interface sur un Ctrl+V : le texte, puis la touche
    // V (scancode 0x2F) enfoncée et relâchée, d'un même élan.
    ws.send(annonce);
    ws.send(new Uint8Array([4, 0x2f, 0x00, 1]));
    ws.send(new Uint8Array([4, 0x2f, 0x00, 0]));
    await browser.waitUntil(
      async () => lignes.includes("annonce du client") && lignes.includes("touche du client"),
      { timeout: 10000, timeoutMsg: `le serveur n'a pas tout reçu : ${JSON.stringify(lignes)}` },
    );
    const ordre = lignes.filter((l) => l === "annonce du client" || l === "touche du client");
    expect(ordre[0]).toBe("annonce du client");
    ws.close();
  });
});
