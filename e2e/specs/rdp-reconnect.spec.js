// Overlay de reconnexion : on connecte un RDP à un serveur DÉDIÉ (port 33898,
// isolé du serveur partagé), on le tue, et l'overlay « Reconnecter / Fermer »
// doit apparaître.
import { startRdpServer, waitForPort } from "./helpers.js";
const PORT = 33898;
let srv;

describe("RDP — overlay de reconnexion à la coupure", () => {
  before(async () => {
    srv = startRdpServer(PORT);
    await waitForPort(PORT);
  });
  after(() => { if (srv) srv.kill(); });

  it("affiche l'overlay quand le serveur coupe", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => {
      const r = document.querySelector('input[name="proto"][value="rdp"]');
      r.checked = true; r.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await $("#m-addr").setValue("127.0.0.1");
    await $("#m-port").setValue(String(PORT));
    await $("#m-user").setValue("test");
    await $("#m-password").setValue("test");
    await $("#m-submit").click();

    await browser.waitUntil(async () => (await $$(".state.live")).length > 0, { timeout: 20000, timeoutMsg: "jamais connecté" });
    // Coupe le serveur -> le WebSocket se ferme -> overlay.
    srv.kill("SIGKILL");
    await $(".rdp-closed").waitForExist({ timeout: 15000, timeoutMsg: "overlay de reconnexion absent" });
    expect(await $('.rdp-closed [data-act="reconnect"]').isExisting()).toBe(true);
    expect(await $('.rdp-closed [data-act="close"]').isExisting()).toBe(true);
  });
});
