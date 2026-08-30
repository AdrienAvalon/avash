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
