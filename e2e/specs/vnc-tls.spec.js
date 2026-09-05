// VeNCrypt : le serveur VNC de test derrière son terminateur TLS. La
// connexion directe joint le port TLS, le client choisit VeNCrypt et le
// sous-type X509Vnc, monte TLS, s'authentifie sous TLS, et le bureau arrive :
// mêmes pixels qu'en clair. Le certificat est épinglé au premier contact
// (fichier des empreintes du bac à sable, clé « vnc:hôte:port ») ; relancé
// avec un autre certificat, le serveur est refusé, et la raison le dit.
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { startVncServer, waitForPort, attendreBureauConnecte } from "./helpers.js";

const VNC_PORT = 35903;
const TLS_PORT = 35904;
const CERT = resolve("../test-rdp-server/cert.pem");
const KEY = resolve("../test-rdp-server/key.pem");

const pixel = (x, y) =>
  browser.execute((px, py) => {
    const c = document.querySelector(".rdp-container canvas");
    const d = c.getContext("2d").getImageData(px, py, 1, 1).data;
    return [d[0], d[1], d[2]];
  }, x, y);

async function connecterVnc(port, motDePasse) {
  await $("#manual-btn").click();
  await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
  await browser.execute(() => {
    const r = document.querySelector('input[name="proto"][value="vnc"]');
    r.checked = true;
    r.dispatchEvent(new Event("change", { bubbles: true }));
  });
  await $("#m-addr").setValue("127.0.0.1");
  await $("#m-port").setValue(String(port));
  await $("#m-password").setValue(motDePasse);
  await $("#m-submit").click();
}

describe("VNC — VeNCrypt, TLS et certificat épinglé", () => {
  let srv;
  let journal = "";

  before(async () => {
    srv = startVncServer(VNC_PORT, (l) => { journal += l; }, { tlsPort: TLS_PORT, cert: CERT, key: KEY });
    await waitForPort(VNC_PORT);
    await waitForPort(TLS_PORT);
  });
  after(() => { if (srv) srv.kill(); });

  it("négocie VeNCrypt X509Vnc, monte TLS, s'authentifie et affiche le bureau", async () => {
    await connecterVnc(TLS_PORT, "test");
    await $(".rdp-container canvas").waitForExist({ timeout: 20000, timeoutMsg: "aucun canvas VNC" });
    await attendreBureauConnecte("le bureau VNC (VeNCrypt)");
    let vu = null;
    await browser.waitUntil(async () => {
      vu = await pixel(100, 100);
      return JSON.stringify(vu) === JSON.stringify([255, 0, 0]);
    }, { timeout: 10000, timeoutMsg: `moitié gauche : ${JSON.stringify(vu)} ; journal : ${journal}` });
    expect(await pixel(540, 380)).toEqual([0, 0, 255]);
    expect(journal).toContain("type 19 choisi");
    expect(journal).toContain("TLS établi");
    // Le certificat est épinglé sous une clé qui dit le protocole.
    const empreintes = join(process.env.AVASH_E2E_SANDBOX, ".config", "avash", "rdp_known_hosts");
    expect(existsSync(empreintes)).toBe(true);
    expect(readFileSync(empreintes, "utf8")).toContain(`vnc:127.0.0.1:${TLS_PORT}`);
  });

  it("un certificat qui change est refusé, et la raison le dit", async () => {
    // L'onglet de la première connexion se ferme d'abord : le serveur qui
    // tombe y poserait sa propre incrustation, et l'on ne saurait plus
    // laquelle on lit.
    await browser.execute(() => document.querySelector(".tab.active .close")?.click());
    await browser.waitUntil(async () => (await $$(".rdp-container")).length === 0, { timeout: 5000 });
    srv.kill();
    srv = null;
    // Un autre certificat, même sujet : seule la clé publique change, et
    // c'est elle qui est épinglée.
    const d = mkdtempSync(join(tmpdir(), "avash-vnc-cert-"));
    execFileSync("openssl", ["req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2", "-subj", "/CN=localhost",
      "-keyout", join(d, "key.pem"), "-out", join(d, "cert.pem")], { stdio: "ignore" });
    journal = "";
    srv = startVncServer(VNC_PORT, (l) => { journal += l; }, { tlsPort: TLS_PORT, cert: join(d, "cert.pem"), key: join(d, "key.pem") });
    await waitForPort(TLS_PORT);
    await connecterVnc(TLS_PORT, "test");
    await browser.waitUntil(async () => (await $$(".rdp-closed")).length > 0,
      { timeout: 20000, timeoutMsg: "le serveur au certificat changé n'a pas été refusé" });
    let texte = "";
    await browser.waitUntil(async () => {
      texte = await browser.execute(() => [...document.querySelectorAll(".toast, .rdp-closed")].map((e) => e.textContent).join(" | "));
      return texte.includes("a changé");
    }, { timeout: 5000, timeoutMsg: `la raison du refus n'est pas affichée ; à l'écran : ${texte}` });
  });
});
