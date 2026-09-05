// Onglets SSH et RDP côte à côte : fermer l'un ne doit pas emporter l'autre.
//
// Signalé en usage réel sous Windows : une session SSH et un bureau RDP ouverts,
// on ferme le SSH, et le bureau devient inutilisable — il fallait fermer son
// onglet et se reconnecter.
import { startRdpServer, waitForPort, findHostRow, attendreBureauConnecte, doubleCliquer } from "./helpers.js";
const RDP_PORT = 33897;
let srv;

describe("Onglets mixtes SSH + RDP", () => {
  before(async () => { srv = startRdpServer(RDP_PORT); await waitForPort(RDP_PORT); });
  after(() => { if (srv) srv.kill(); });

  it("fermer la session SSH rend la main au bureau RDP, utilisable", async () => {
    // 1. Un bureau RDP.
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => {
      const r = document.querySelector('input[name="proto"][value="rdp"]');
      r.checked = true; r.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await $("#m-addr").setValue("127.0.0.1");
    await $("#m-port").setValue(String(RDP_PORT));
    await $("#m-user").setValue("test");
    await $("#m-password").setValue("test");
    await $("#m-submit").click();
    await attendreBureauConnecte();

    // 2. Une session SSH réelle par-dessus, qui devient l'onglet actif.
    const ligne = await findHostRow("test-ssh");
    await doubleCliquer(ligne);
    await browser.waitUntil(async () => (await $$(".tab")).length === 2,
      { timeout: 20000, timeoutMsg: "second onglet absent" });
    await browser.waitUntil(
      async () => browser.execute(() => {
        const c = document.querySelector(".rdp-container");
        return c ? getComputedStyle(c).display === "none" : false;
      }),
      { timeout: 8000, timeoutMsg: "le bureau RDP aurait dû être masqué par le SSH" },
    );

    // 3. On ferme l'onglet SSH — celui qui est actif.
    await browser.execute(() => {
      const actif = document.querySelector(".tab.active .close");
      actif.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    // 4. Le bureau doit redevenir visible et redevenir l'onglet actif.
    await browser.waitUntil(
      async () => browser.execute(() => {
        const c = document.querySelector(".rdp-container");
        return !!c && getComputedStyle(c).display !== "none";
      }),
      { timeout: 8000, timeoutMsg: "le bureau RDP n'a pas été réaffiché" },
    );
    expect(await $(".tab.active").isExisting()).toBe(true);
    // Et il doit être utilisable : le canvas prend le focus clavier, sans quoi
    // les frappes ne partent nulle part.
    const focalise = await browser.execute(() =>
      document.activeElement?.tagName === "CANVAS");
    expect(focalise).toBe(true);
    // Enfin, la session ne doit pas s'être fermée pendant l'opération.
    expect(await $(".rdp-closed").isExisting()).toBe(false);
  });
});
