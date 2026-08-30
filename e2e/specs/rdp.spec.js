// Connexion RDP réelle contre un serveur de test DÉDIÉ (test/test sur 33899).
// Valide toute la chaîne : formulaire → sidecar → WebSocket → rendu du canvas.
import { startRdpServer } from "./helpers.js";
const RDP_PORT = 33899;
let srv;

describe("RDP — connexion réelle au serveur de test", () => {
  before(() => { srv = startRdpServer(RDP_PORT); });
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
    await browser.waitUntil(async () => (await $$(".state.live")).length > 0,
      { timeout: 20000, timeoutMsg: "jamais connecté (handshake RDP échoué ?)" });
    expect(await $(".rdp-closed").isExisting()).toBe(false);
  });
});
