// Son du bureau distant : le serveur de test joue un la 440 Hz dès que le canal
// audio est négocié ; le sidecar, piloté ici directement sur son WebSocket
// (comme rdp-fichiers.spec.js), relaie les blocs PCM en messages [20]. On
// vérifie le format négocié (PCM 16 bits, 44,1 kHz, stéréo), un débit proche
// du temps réel, et que ce n'est pas du silence. Et qu'avec --sans-son, rien
// ne vient.
import { spawn } from "node:child_process";
import { resolve } from "node:path";
import { waitForPort } from "./helpers.js";

const PORT = 33897;
const MSG_CONNECTE = 1;
const MSG_SON = 20;

/** Lance le sidecar sur le serveur de test et rend son WebSocket connecté. */
async function connecter(argsSup) {
  const sidecar = spawn(
    resolve("../rdp-sidecar/target/release/avash-rdp"),
    ["--host", "127.0.0.1", "--port", String(PORT), "-u", "test", "--width", "800", "--height", "600", ...argsSup],
    { stdio: ["pipe", "pipe", "ignore"] },
  );
  sidecar.stdin.write("test\n");
  const [wsPort, token] = await new Promise((ok, ko) => {
    const t = setTimeout(() => ko(new Error("le sidecar n'a rien annoncé")), 15000);
    sidecar.stdout.once("data", (b) => { clearTimeout(t); ok(b.toString().trim().split(/\s+/)); });
  });
  const ws = new WebSocket(`ws://127.0.0.1:${wsPort}`);
  ws.binaryType = "arraybuffer";
  const blocs = [];
  const connecte = new Promise((ok, ko) => {
    const t = setTimeout(() => ko(new Error("connexion RDP jamais annoncée")), 25000);
    ws.onmessage = (ev) => {
      const octets = new Uint8Array(ev.data);
      if (octets[0] === MSG_CONNECTE) { clearTimeout(t); ok(); }
      if (octets[0] === MSG_SON) blocs.push(new DataView(ev.data));
    };
  });
  ws.onopen = () => ws.send(new TextEncoder().encode(token));
  await connecte;
  return { sidecar, ws, blocs };
}

describe("RDP — son du bureau distant", () => {
  let srv;
  before(async () => {
    srv = spawn("./target/release/test-rdp-server",
      ["--bind-addr", `127.0.0.1:${PORT}`, "--cert", "cert.pem", "--key", "key.pem",
       "--user", "test", "--pass", "test", "--sec", "hybrid"],
      { cwd: resolve("../test-rdp-server"), stdio: "ignore" });
    await waitForPort(PORT);
  });
  after(() => { if (srv) srv.kill(); });

  it("relaie le PCM négocié, en temps réel, et ce n'est pas du silence", async () => {
    const { sidecar, ws, blocs } = await connecter([]);
    try {
      const debut = Date.now();
      await browser.waitUntil(() => blocs.length >= 25, { timeout: 10000, timeoutMsg: "moins de 25 blocs audio en 10 s" });
      const duree = (Date.now() - debut) / 1000;
      let echantillons = 0;
      let nonNuls = 0;
      for (const dv of blocs) {
        expect(dv.getUint32(6, true)).toBe(44100);
        expect(dv.getUint8(10)).toBe(2);
        expect(dv.getUint8(11)).toBe(16);
        const octets = dv.byteLength - 12;
        expect(octets % 4).toBe(0);
        echantillons += octets / 4;
        for (let i = 12; i < dv.byteLength; i += 2) if (dv.getInt16(i, true) !== 0) nonNuls += 1;
      }
      // Le serveur pousse par tranches de 20 ms : 25 blocs font une demi-seconde.
      const secondes = echantillons / 44100;
      expect(secondes).toBeGreaterThan(0.4);
      expect(secondes).toBeLessThan(duree + 1);
      expect(nonNuls).toBeGreaterThan(echantillons);
    } finally {
      ws.close();
      sidecar.kill();
    }
  });

  it("avec --sans-son, aucun bloc ne vient", async () => {
    const { sidecar, ws, blocs } = await connecter(["--sans-son"]);
    try {
      await browser.pause(1500);
      expect(blocs.length).toBe(0);
    } finally {
      ws.close();
      sidecar.kill();
    }
  });
});
