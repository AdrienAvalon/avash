describe("Modales & raccourcis", () => {
  it("la modale « Connexion directe » ne se ferme PAS au clic en dehors", async () => {
    await $("#manual-btn").click();
    const modal = await $("#manual-modal");
    await modal.waitForDisplayed({ timeout: 5000 });
    // Clic en haut à gauche = sur le fond (le formulaire est centré) : doit rester.
    await browser.action("pointer").move({ x: 6, y: 6, origin: "viewport" }).down().pause(30).up().perform();
    await browser.pause(250);
    await expect(modal).toBeDisplayed();
  });

  it("… mais se ferme à Échap", async () => {
    await browser.keys("Escape");
    await browser.pause(250);
    await expect($("#manual-modal")).not.toBeDisplayed();
  });

  it("la palette s'ouvre à Ctrl+K et se ferme à Échap", async () => {
    await browser.keys(["Control", "k"]);
    const palette = await $("#palette");
    await palette.waitForDisplayed({ timeout: 5000 });
    await browser.keys("Escape");
    await browser.pause(250);
    await expect(palette).not.toBeDisplayed();
  });
});
