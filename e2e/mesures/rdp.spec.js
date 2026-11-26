// Mesure de l'affichage d'un bureau distant, sur la vraie application : le
// serveur de test pousse une mire plein écran (1920×1080) qui change
// entièrement à chaque image, à la cadence MIRE_HZ (30 par défaut). On relève
// pendant FENETRE_MS ce que le processus RDP affiche dans son bandeau
// (images par seconde, débit vers l'interface, aller-retour d'une image
// jusqu'à l'accusé de la page) et le temps processeur de chaque processus.
//
// L'aller-retour isole le coût côté poste : envoi par le WebSocket local,
// décodage de la trame, putImageData. La cadence dit si la chaîne suit.
//
// Ce n'est pas un test : pas d'assertion, les chiffres vont dans
// e2e/mesures/resultat-rdp.json et docs/feuille-de-route.md.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { startRdpServer, waitForPort, attendreBureauConnecte } from "../specs/helpers.js";

const PORT = 33911;
const HZ = Number(process.env.MIRE_HZ ?? 30);
const CHAUFFE_MS = 3000;
const FENETRE_MS = 10000;
const TIC = Number(execFileSync("getconf", ["CLK_TCK"]).toString().trim());
let srv;

/** Temps processeur cumulé (utilisateur + système), en secondes, du processus. */
function cpu(pid) {
  try {
    const champs = readFileSync(`/proc/${pid}/stat`, "utf8").split(") ")[1].split(" ");
    return (Number(champs[11]) + Number(champs[12])) / TIC;
  } catch {
    return null;
  }
}

/** Le plus récent processus portant ce nom exact, ou null. */
function pidDe(nom) {
  try {
    return Number(execFileSync("pgrep", ["-nx", nom]).toString().trim());
  } catch {
    return null;
  }
}

function stats(valeurs) {
  const t = [...valeurs].sort((a, b) => a - b);
  const q = (p) => t[Math.min(t.length - 1, Math.floor(p * t.length))];
  return { mediane: q(0.5), p95: q(0.95), min: t[0], max: t.at(-1), n: t.length };
}

describe("Mesure de l'affichage RDP", () => {
  before(async () => { srv = startRdpServer(PORT, null, ["--mire", String(HZ)]); await waitForPort(PORT); });
  after(() => { if (srv) srv.kill(); });

  it(`mire plein écran à ${HZ} images par seconde`, async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => {
      const r = document.querySelector('input[name="proto"][value="rdp"]');
      r.checked = true;
      r.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await $("#m-addr").setValue("127.0.0.1");
    await $("#m-port").setValue(String(PORT));
    await $("#m-user").setValue("test");
    await $("#m-password").setValue("test");
    await $("#m-submit").click();
    await attendreBureauConnecte();
    await browser.pause(CHAUFFE_MS);

    const noms = { rdp: "avash-rdp", page: "WebKitWebProces", appli: "avash-ui", serveur: "test-rdp-server" };
    const pids = Object.fromEntries(Object.entries(noms).map(([k, n]) => [k, pidDe(n)]));
    const avant = Object.fromEntries(Object.entries(pids).map(([k, p]) => [k, p ? cpu(p) : null]));
    const t0 = Date.now();
    const releves = [];
    while (Date.now() - t0 < FENETRE_MS) {
      await browser.pause(500);
      const hud = await browser.execute(() => document.querySelector(".rdp-hud")?.textContent ?? "");
      const m = hud.match(/(\d+)\s*fps\s*·\s*([\d.]+)\s*(Ko|Mo)\/s\s*·\s*(\d+)\s*ms/);
      if (m) releves.push({ fps: Number(m[1]), moParS: m[3] === "Mo" ? Number(m[2]) : Number(m[2]) / 1024, ms: Number(m[4]) });
    }
    const duree = (Date.now() - t0) / 1000;
    const processeur = Object.fromEntries(Object.entries(pids).map(([k, p]) => {
      const fin = p ? cpu(p) : null;
      return [k, fin !== null && avant[k] !== null ? Math.round(((fin - avant[k]) / duree) * 100) : null];
    }));
    const resultat = {
      cadenceDemandee: HZ,
      fps: stats(releves.map((r) => r.fps)),
      moParS: stats(releves.map((r) => r.moParS)),
      allerRetourMs: stats(releves.map((r) => r.ms)),
      processeurPourcentDunCoeur: processeur,
    };
    console.log(`\nAffichage RDP, mire 1920×1080 à ${HZ} i/s (${releves.length} relevés sur ${duree.toFixed(1)} s) :`);
    console.log(`  images affichées par seconde : médiane ${resultat.fps.mediane}, min ${resultat.fps.min}`);
    console.log(`  débit vers la page : médiane ${resultat.moParS.mediane.toFixed(1)} Mo/s`);
    console.log(`  aller-retour d'une image (envoi → accusé de la page) : médiane ${resultat.allerRetourMs.mediane} ms, p95 ${resultat.allerRetourMs.p95} ms`);
    console.log(`  processeur, en % d'un cœur : ${Object.entries(processeur).map(([k, v]) => `${k} ${v}`).join(", ")}`);
    writeFileSync(join(import.meta.dirname, `resultat-rdp-${HZ}.json`), JSON.stringify(resultat, null, 2) + "\n");
  });
});
