// Connexion RDP réelle contre un serveur de test DÉDIÉ (test/test sur 33899).
// Valide toute la chaîne : formulaire → sidecar → WebSocket → rendu du canvas.
import { startRdpServer, waitForPort, attendreBureauConnecte } from "./helpers.js";
const RDP_PORT = 33899;
let srv;

describe("RDP — connexion réelle au serveur de test", () => {
  before(async () => { srv = startRdpServer(RDP_PORT); await waitForPort(RDP_PORT); });
  after(() => { if (srv) srv.kill(); });

  it("se connecte et affiche le bureau (canvas rendu)", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => {
      const r = document.querySelector('input[name="proto"][value="rdp"]');
      r.checked = true;
      r.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await $("#m-addr").setValue("127.0.0.1");
    await $("#m-port").setValue(String(RDP_PORT));
    await $("#m-user").setValue("test");
    await $("#m-password").setValue("test");
    await $("#m-submit").click();

    await $(".rdp-container canvas").waitForExist({ timeout: 20000, timeoutMsg: "aucun canvas RDP" });
    // Signal fiable d'une vraie connexion : l'onglet passe à .state.live seulement
    // après le message « connecté » du sidecar (handshake CredSSP réussi).
    await attendreBureauConnecte();
    expect(await $(".rdp-closed").isExisting()).toBe(false);
  });
});

// Audit du 27 novembre 2026 (QT-13) : le scénario ci-dessus annonce un « canvas
// rendu » mais ne regarde aucun pixel ; seuls les scénarios VNC lisaient
// l'image. Toute la chaîne processus RDP, WebSocket, `putImageData` sous
// WebKitGTK et WebView2 n'était donc pas jugée sur ce qu'elle affiche : un
// bureau noir ou aux canaux inversés passait. Le serveur de test pousse ici sa
// mire (`--mire`), dont le canal rouge ne dépend que de la ligne :
// `(y / 4) & 0xff`, éclairci par `| 0xc0` dans un bandeau qui défile. Sur les
// lignes où ce rouge a déjà ses deux bits de poids fort, le bandeau ne change
// rien : 0xc5 en y = 788, 0xd0 en y = 832, quelle que soit l'image. Le vert et
// le bleu, eux, glissent d'une image à l'autre. Une petite tolérance couvre un
// codec avec pertes.
const PORT_MIRE = 33903;
let mire;

/** Les composantes RGBA d'un pixel du bureau, lues dans le canevas. */
const pixelRdp = (x, y) =>
  browser.execute((px, py) => {
    // Le bureau du scénario précédent reste ouvert, fermé : on lit le canevas
    // affiché, celui de l'onglet actif.
    const c = [...document.querySelectorAll(".rdp-container canvas")].find((k) => k.offsetParent !== null);
    if (!c || c.width < 1920 || c.height < 1080) return null;
    return [...c.getContext("2d").getImageData(px, py, 1, 1).data];
  }, x, y);

describe("RDP — l'image du serveur arrive dans le canevas", () => {
  before(async () => { mire = startRdpServer(PORT_MIRE, null, ["--mire", "2"]); await waitForPort(PORT_MIRE); });
  after(() => { if (mire) mire.kill(); });

  it("le bureau rdp affiche la mire du serveur", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => {
      const r = document.querySelector('input[name="proto"][value="rdp"]');
      r.checked = true;
      r.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await $("#m-addr").setValue("127.0.0.1");
    await $("#m-port").setValue(String(PORT_MIRE));
    await $("#m-user").setValue("test");
    await $("#m-password").setValue("test");
    await $("#m-submit").click();
    await attendreBureauConnecte("le bureau de la mire");

    const attendus = [
      { x: 100, y: 788, rouge: 0xc5 },
      { x: 1500, y: 790, rouge: 0xc5 },
      { x: 300, y: 832, rouge: 0xd0 },
      { x: 1800, y: 835, rouge: 0xd0 },
    ];
    let vus = [];
    try {
      await browser.waitUntil(async () => {
        vus = await Promise.all(attendus.map(({ x, y }) => pixelRdp(x, y)));
        return vus.every((p, i) => p !== null && Math.abs(p[0] - attendus[i].rouge) <= 6 && p[3] === 255);
      }, { timeout: 15000, interval: 300 });
    } catch (e) {
      throw new Error(`la mire n'est pas dans le canevas : ${JSON.stringify(vus)}, rouges attendus ${attendus.map((a) => a.rouge).join(", ")}`, { cause: e });
    }
  });
});
