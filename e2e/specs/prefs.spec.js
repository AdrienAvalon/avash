// Réglage du partage de presse-papiers avec les bureaux RDP.
//
// Ce réglage décide si le contenu du presse-papiers local est offert à un
// serveur distant. Il n'est atteignable qu'à la palette : on vérifie ici qu'il
// y est bien, qu'il bascule, qu'il est retenu, et que son libellé dit l'état
// courant plutôt que l'état visé — la confusion classique d'un interrupteur.
describe("Presse-papiers RDP — réglage de partage", () => {
  const CLE = "avash.rdp.clipboard";

  const ouvrirPalette = async () => {
    await browser.keys(["Control", "k"]);
    await $("#palette").waitForDisplayed({ timeout: 5000 });
  };
  const fermerPalette = async () => {
    await browser.keys(["Escape"]);
    await browser.waitUntil(async () => !(await $("#palette").isDisplayed()), { timeout: 5000 });
  };
  // Première entrée de la palette : les réglages précèdent les hôtes.
  const libelle = async () => $("#palette-results .item .name").getProperty("textContent");
  const reglage = async () => browser.execute((k) => localStorage.getItem(k), CLE);

  before(async () => {
    await browser.execute((k) => localStorage.removeItem(k), CLE);
  });

  it("propose la bascule à la palette, en annonçant l'état courant", async () => {
    await ouvrirPalette();
    // Rien n'a jamais été réglé : le partage est actif, la commande propose donc
    // de l'arrêter.
    expect(await libelle()).toBe("Ne plus partager le presse-papiers avec les bureaux RDP");
    await fermerPalette();
  });

  it("coupe le partage et le retient", async () => {
    await ouvrirPalette();
    await $("#palette-results .item").click();
    await browser.waitUntil(async () => (await reglage()) === "0",
      { timeout: 5000, timeoutMsg: "le refus n'a pas été enregistré" });

    // Le libellé doit avoir suivi : il propose maintenant de rétablir.
    await ouvrirPalette();
    expect(await libelle()).toBe("Partager le presse-papiers avec les bureaux RDP");
    await fermerPalette();
  });

  it("rétablit le partage", async () => {
    await ouvrirPalette();
    await $("#palette-results .item").click();
    await browser.waitUntil(async () => (await reglage()) === "1",
      { timeout: 5000, timeoutMsg: "le partage n'a pas été rétabli" });
  });
});
