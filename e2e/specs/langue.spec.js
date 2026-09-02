// Langue de l'interface.
//
// Le français est la langue source ; l'anglais se choisit dans la palette et
// se mémorise. Le scénario bascule, vérifie que la page et les textes produits
// par le code suivent, puis revient au français : les autres scénarios
// affirment des textes français.
describe("Langue de l'interface", () => {
  const texte = (sel) => browser.execute((s) => document.querySelector(s)?.textContent?.trim() ?? null, sel);

  async function commandePalette(fragment) {
    await browser.keys(["Control", "k"]);
    const input = await $("#palette-input");
    await input.waitForDisplayed({ timeout: 5000 });
    await input.setValue(fragment);
    const item = await $("#palette-results .item");
    await item.waitForDisplayed({ timeout: 5000 });
    await item.click();
  }

  it("passe en anglais depuis la palette : page, infobulles et accueil suivent", async () => {
    expect(await texte("#manual-btn")).toBe("Connexion directe");
    await commandePalette("Switch to English");
    await browser.waitUntil(async () => (await texte("#manual-btn")) === "Direct connection", {
      timeout: 5000, timeoutMsg: "la barre latérale n'est pas passée en anglais",
    });
    expect(await browser.execute(() => document.documentElement.lang)).toBe("en");
    expect(await texte("#keys-btn")).toBe("My SSH keys");
    expect(await texte("#empty-hint")).toBe("Double-click a host to connect");
    expect(await browser.execute(() => document.querySelector("#search").placeholder)).toBe("Filter hosts…");
    expect(await browser.execute(() => localStorage.getItem("avash.langue"))).toBe("en");
  });

  it("revient au français, et le choix est mémorisé", async () => {
    await commandePalette("Passer en français");
    await browser.waitUntil(async () => (await texte("#manual-btn")) === "Connexion directe", { timeout: 5000 });
    expect(await texte("#empty-hint")).toBe("Double-clic sur un hôte pour te connecter");
    expect(await browser.execute(() => localStorage.getItem("avash.langue"))).toBe("fr");
  });
});
