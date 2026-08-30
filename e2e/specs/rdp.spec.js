// Connexion RDP réelle contre le serveur de test (démarré par wdio.conf onPrepare,
// identifiants test/test sur 127.0.0.1:33899). Valide toute la chaîne : formulaire
// → sidecar → WebSocket → rendu du canvas.
const RDP_PORT = 33899;

describe("RDP — connexion réelle au serveur de test", () => {
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

    // Le canvas RDP existe.
    await $(".rdp-container canvas").waitForExist({ timeout: 20000, timeoutMsg: "aucun canvas RDP" });
    // Signal FIABLE d'une vraie connexion : l'onglet passe à .state.live seulement
    // après réception du message « connecté » du sidecar (handshake CredSSP réussi).
    await browser.waitUntil(
      async () => (await $$(".state.live")).length > 0,
      { timeout: 20000, timeoutMsg: "jamais connecté (handshake RDP échoué ?)" },
    );
    // Et surtout : aucun overlay d'échec.
    expect(await $(".rdp-closed").isExisting()).toBe(false);
  });
});
