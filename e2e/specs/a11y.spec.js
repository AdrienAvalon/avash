// Accessibilité : rôles ARIA des boîtes de dialogue et comportement du focus.
// Vérifié sur l'application réelle — c'est le seul endroit où le focus se
// comporte comme chez l'utilisateur.

describe("Accessibilité des boîtes de dialogue", () => {
  it("la modale porte role=dialog et un titre accessible existant", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    const box = await $('#manual-modal [role="dialog"]');
    expect(await box.isExisting()).toBe(true);
    expect(await box.getAttribute("aria-modal")).toBe("true");
    // aria-labelledby doit pointer sur un élément réellement présent.
    const id = await box.getAttribute("aria-labelledby");
    expect(await $(`#${id}`).isExisting()).toBe(true);
    await browser.keys("Escape");
  });

  it("Tab reste enfermé dans la modale (ne fuit pas vers la page derrière)", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    // Assez de Tab pour faire plusieurs fois le tour de la boîte.
    for (let i = 0; i < 25; i++) await browser.keys("Tab");
    const dansLaModale = await browser.execute(
      () => !!document.activeElement?.closest("#manual-modal"),
    );
    expect(dansLaModale).toBe(true);
    await browser.keys("Escape");
  });

  it("le focus revient au bouton déclencheur après fermeture", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.keys("Escape");
    await browser.waitUntil(
      async () => browser.execute(() => document.activeElement?.id === "manual-btn"),
      { timeout: 5000, timeoutMsg: "le focus n'est pas revenu sur le déclencheur" },
    );
  });

  it("les boutons icône-seule ont un nom accessible", async () => {
    const sans = await browser.execute(() =>
      [...document.querySelectorAll("button")]
        .filter((b) => !b.textContent.trim() && b.querySelector("svg"))
        .filter((b) => !b.getAttribute("aria-label") && !b.getAttribute("title"))
        .map((b) => b.id || b.className),
    );
    expect(sans).toEqual([]);
  });

  // Trouvé par l'audit du 7 septembre 2026 : `.modal input:focus { outline:
  // none }` écrasait l'anneau :focus-visible des radios cachées (0×0) des
  // interrupteurs segmentés. En Maj+Tab depuis « Adresse ou IP » le focus
  // arrivait sur la radio SSH sans qu'aucun indicateur n'apparaisse (WCAG
  // 2.4.7). On met l'état complet DANS l'assertion (comme axe.spec.js) : un
  // simple booléen rendrait un échec de CI indéchiffrable, et closest() renvoie
  // null si le focus n'est pas où on croit.
  it("l'interrupteur segmenté montre le focus clavier", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.keys(["Shift", "Tab"]);
    const etat = await browser.execute(() => {
      const a = document.activeElement;
      const pastille = a && a.closest(".auth-switch .radio");
      if (!pastille) return { type: a && a.type, trouve: false };
      return { type: a.type, trouve: true, contour: getComputedStyle(pastille).outlineStyle };
    });
    expect(etat).toEqual({ type: "radio", trouve: true, contour: "solid" });
    await browser.keys("Escape");
  });
});
