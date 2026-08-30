// Redimensionner la fenêtre ne doit pas figer l'interface, même sans session.
// La barre de titre est maison : chaque redimensionnement déclenchait un
// aller-retour vers le backend, qui saturait le pont au bout de quelques
// secondes de glissé. Ce scénario rejoue une rafale et vérifie que l'application
// répond toujours — et dans un délai raisonnable.
describe("Redimensionnement de la fenêtre", () => {
  it("reste répondante après une rafale de redimensionnements", async () => {
    await $("#host-list").waitForExist({ timeout: 8000 });

    for (let i = 0; i < 30; i++) {
      await browser.setWindowSize(900 + (i % 12) * 25, 650 + (i % 8) * 20);
    }

    // L'interface doit encore répondre : on mesure le temps d'un aller-retour.
    const debut = Date.now();
    const vivant = await browser.execute(() => !!document.getElementById("host-list"));
    const duree = Date.now() - debut;
    expect(vivant).toBe(true);
    expect(duree).toBeLessThan(4000); // figée, la webview ne répondrait pas
  });
});
