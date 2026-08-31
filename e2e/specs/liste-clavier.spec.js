// La barre latérale au clavier.
//
// Les lignes d'hôte étaient de simples `div` : la connexion passait par un
// double-clic, les options par un clic droit. Tab sautait du champ de recherche
// aux boutons du bas — la liste entière était hors d'atteinte, et l'on ne
// pouvait ni éditer, ni déplacer, ni supprimer un hôte sans souris.
describe("Liste d'hôtes — atteignable au clavier", () => {
  const lignes = () => $$("#host-list [role='button']");

  // La liste est peuplée par un appel asynchrone au démarrage : attendre l'état,
  // jamais une durée.
  before(async () => {
    await browser.waitUntil(async () => (await lignes()).length > 0,
      { timeout: 10000, timeoutMsg: "aucune ligne d'hôte focalisable" });
  });

  it("chaque hôte est focalisable et annoncé comme un bouton", async () => {
    const l = await lignes();
    expect(l.length).toBeGreaterThan(0);
    for (const ligne of l) {
      expect(await ligne.getAttribute("tabindex")).toBe("0");
    }
  });

  it("les flèches déplacent le focus d'une ligne à l'autre", async () => {
    await browser.execute(() => (document.querySelector("#host-list [role='button']")).focus());

    const alias = async () =>
      browser.execute(() => document.activeElement?.querySelector(".alias")?.textContent ?? null);

    const premier = await alias();
    expect(premier).not.toBe(null);

    await browser.keys(["ArrowDown"]);
    const second = await alias();
    expect(second).not.toBe(premier);

    await browser.keys(["ArrowUp"]);
    expect(await alias()).toBe(premier);
  });

  it("Maj+F10 ouvre le menu contextuel de la ligne focalisée", async () => {
    await browser.execute(() => (document.querySelector("#host-list [role='button']")).focus());
    await browser.keys(["Shift", "F10"]);
    await $("#host-context").waitForDisplayed({ timeout: 5000, timeoutMsg: "menu contextuel absent au clavier" });
    await browser.keys(["Escape"]);
  });
});
