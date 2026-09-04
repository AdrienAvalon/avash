// Fichiers par le presse-papiers RDP, dans les deux sens.
//
// Comme `rdp-clipboard.spec.js`, ce scénario pilote le sidecar `avash-rdp`
// directement sur son WebSocket : la chaîne réelle (serveur de test CLIPRDR →
// IronRDP → sidecar → trames JSON du front, et retour) sans dépendre du
// presse-papiers du système ni d'un bureau. Les octets sont comparés de bout
// en bout, sur un fichier assez gros pour traverser plusieurs morceaux.
import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { randomBytes } from "node:crypto";
import { waitForPort } from "./helpers.js";

const PORT = 33896;
const MSG_CONNECTE = 1;
const MSG_PRESSE_PAPIERS = 8;
const MSG_FICHIERS_DISTANTS = 15;
const MSG_RECEVOIR = 16;
const MSG_PROGRESSION = 17;
const MSG_TERMINE = 18;
const MSG_OFFRIR = 19;
const DECLENCHEUR = "avash-offre-fichiers"; // cf. DECLENCHEUR_OFFRE du serveur de test

describe("RDP — fichiers par le presse-papiers", () => {
  let srv;
  let sidecar;
  let journal = "";
  const racine = mkdtempSync(join(tmpdir(), "avash-fichiers-"));
  const recuParLeServeur = join(racine, "serveur");
  const recuParLePoste = join(racine, "poste");
  // Assez gros pour plusieurs morceaux d'un mégaoctet côté poste, et
  // plusieurs de 64 Kio côté serveur ; aléatoire pour qu'un décalage se voie.
  const offertParLeServeur = join(racine, "du-serveur.bin");
  const offertParLePoste = join(racine, "du-poste.bin");
  writeFileSync(offertParLeServeur, randomBytes(2_500_000 + 123));
  writeFileSync(offertParLePoste, randomBytes(300_000 + 7));

  before(async () => {
    srv = spawn("./target/release/test-rdp-server",
      ["--bind-addr", `127.0.0.1:${PORT}`, "--cert", "cert.pem", "--key", "key.pem",
       "--user", "test", "--pass", "test", "--sec", "hybrid",
       "--offrir", offertParLeServeur, "--recevoir-dans", recuParLeServeur],
      { cwd: resolve("../test-rdp-server"), stdio: ["ignore", "pipe", "ignore"] });
    srv.stdout.setEncoding("utf8");
    srv.stdout.on("data", (d) => { journal += String(d); });
    await waitForPort(PORT);
  });
  after(() => {
    if (sidecar) sidecar.kill();
    if (srv) srv.kill();
  });

  it("le poste reçoit le fichier copié sur le distant, et le distant reçoit celui du poste", async () => {
    sidecar = spawn(
      resolve("../rdp-sidecar/target/release/avash-rdp"),
      ["--host", "127.0.0.1", "--port", String(PORT), "-u", "test", "--width", "1024", "--height", "768"],
      { stdio: ["pipe", "pipe", "ignore"] },
    );
    sidecar.stdin.write("test\n");
    const [wsPort, token] = await new Promise((ok, ko) => {
      const t = setTimeout(() => ko(new Error("le sidecar n'a rien annoncé")), 15000);
      sidecar.stdout.once("data", (b) => { clearTimeout(t); ok(b.toString().trim().split(/\s+/)); });
    });

    const ws = new WebSocket(`ws://127.0.0.1:${wsPort}`);
    ws.binaryType = "arraybuffer";
    // Les trames reçues, par code, et des promesses qui attendent la prochaine.
    const attentes = new Map();
    const attendre = (code, delai, quoi) => new Promise((ok, ko) => {
      const t = setTimeout(() => ko(new Error(`${quoi} : rien reçu en ${delai} ms ; journal du serveur : ${journal.replace(/\n/g, " / ")}`)), delai);
      attentes.set(code, (payload) => { clearTimeout(t); ok(payload); });
    });
    const envoyerJson = (code, valeur) => {
      const corps = new TextEncoder().encode(JSON.stringify(valeur));
      const m = new Uint8Array(1 + corps.length);
      m[0] = code;
      m.set(corps, 1);
      ws.send(m);
    };
    ws.onmessage = (ev) => {
      const octets = new Uint8Array(ev.data);
      const code = octets[0];
      const suite = attentes.get(code);
      if (!suite) return;
      if (code === MSG_TERMINE || code === MSG_FICHIERS_DISTANTS || code === MSG_PROGRESSION) {
        attentes.delete(code);
        suite(JSON.parse(new TextDecoder().decode(octets.slice(1))));
      } else {
        attentes.delete(code);
        suite(octets);
      }
    };
    const connecte = attendre(MSG_CONNECTE, 25000, "connexion RDP");
    ws.onopen = () => ws.send(new TextEncoder().encode(token));
    await connecte;
    // Le texte initial du serveur passe d'abord : le canal est prêt.
    await attendre(MSG_PRESSE_PAPIERS, 15000, "presse-papiers initial");

    // 1) Distant → poste. Le poste « copie » le texte déclencheur : le serveur
    //    de test répond en copiant son fichier ; le sidecar en demande la liste
    //    (jamais le contenu) et la présente.
    const liste = attendre(MSG_FICHIERS_DISTANTS, 15000, "liste des fichiers distants");
    const corps = new TextEncoder().encode(DECLENCHEUR);
    const m8 = new Uint8Array(1 + corps.length);
    m8[0] = MSG_PRESSE_PAPIERS;
    m8.set(corps, 1);
    ws.send(m8);
    const annonce = await liste;
    expect(annonce.fichiers).toHaveLength(1);
    expect(annonce.fichiers[0].chemin).toBe(basename(offertParLeServeur));
    expect(annonce.fichiers[0].taille).toBe(2_500_000 + 123);
    expect(annonce.fichiers[0].dossier).toBe(false);
    expect(typeof annonce.dossier).toBe("string");

    // Rien n'a été écrit avant l'accord.
    expect(existsSync(join(recuParLePoste, basename(offertParLeServeur)))).toBe(false);
    const fin = attendre(MSG_TERMINE, 30000, "fin de réception");
    envoyerJson(MSG_RECEVOIR, { dossier: recuParLePoste });
    const bilan = await fin;
    expect(bilan.sens).toBe("reception");
    expect(bilan.erreurs).toEqual([]);
    expect(bilan.fichiers).toBe(1);
    expect(bilan.octets).toBe(2_500_000 + 123);
    const recu = readFileSync(join(recuParLePoste, basename(offertParLeServeur)));
    expect(recu.equals(readFileSync(offertParLeServeur))).toBe(true);
    expect(existsSync(join(recuParLePoste, `${basename(offertParLeServeur)}.part`))).toBe(false);

    // 2) Poste → distant. Le poste offre un fichier ; le serveur de test, qui
    //    joue l'utilisateur distant, en demande la liste puis le contenu.
    const offre = attendre(MSG_TERMINE, 15000, "accusé de l'offre");
    envoyerJson(MSG_OFFRIR, [offertParLePoste]);
    const accuse = await offre;
    expect(accuse.sens).toBe("offre");
    expect(accuse.erreurs).toEqual([]);
    expect(accuse.fichiers).toBe(1);
    expect(accuse.octets).toBe(300_000 + 7);
    const debut = Date.now();
    while (!journal.includes("réception terminée")) {
      if (Date.now() - debut > 30000) throw new Error(`le serveur n'a pas fini de recevoir ; journal : ${journal.replace(/\n/g, " / ")}`);
      await new Promise((r) => setTimeout(r, 100));
    }
    const chezLeServeur = readFileSync(join(recuParLeServeur, basename(offertParLePoste)));
    expect(chezLeServeur.equals(readFileSync(offertParLePoste))).toBe(true);
    ws.close();
  });
});
