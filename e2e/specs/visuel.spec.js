// Régression visuelle : l'interface est comparée pixel à pixel à des captures
// de référence, sur les deux thèmes et sur les surfaces qui bougent le plus.
//
// Ce que les autres scénarios ne voient pas : une marge qui saute, une couleur
// de jeton qui change, un texte qui déborde. Les références vivent dans
// e2e/visuel/reference et sont celles de la chaîne d'intégration. Le scénario
// ne tourne qu'avec VISUEL=1, dans un passage à part (voir wdio.conf.js).
describe("Régression visuelle", () => {
  before(async function () {
    // Les références sont celles de Linux (ubuntu-latest) : ailleurs, les
    // polices ne rendent pas pareil et la comparaison ne dirait rien.
    if (!process.env.VISUEL || process.platform !== "linux") this.skip();
    // Une taille fixe : les captures dépendent de la géométrie.
    try { await browser.setWindowSize(1280, 800); } catch { /* pilote sans redimensionnement */ }
    await browser.waitUntil(async () => (await $$("#host-list [data-cle]")).length > 0, { timeout: 10000 });
    await browser.execute(() => {
      // Geler ce qui bouge : animations, curseur clignotant, transitions.
      const s = document.createElement("style");
      s.textContent = "*,*::before,*::after{transition:none!important;animation:none!important;caret-color:transparent!important}";
      document.head.appendChild(s);
    });
    await browser.pause(300);
  });

  const seuil = 0.5; // pour-cent de pixels différents tolérés (anticrénelage)

  const theme = async (voulu) => {
    for (let i = 0; i < 4; i++) {
      const actuel = await browser.execute(() => document.documentElement.getAttribute("data-theme"));
      if (actuel === voulu) return;
      await $("#theme-toggle").click();
    }
  };

  it("accueil, thème sombre", async () => {
    await theme("dark");
    await browser.pause(200);
    expect(await browser.checkScreen("accueil-sombre")).toBeLessThanOrEqual(seuil);
  });

  // Chaque test remet l'interface en état AVANT de juger sa capture. Trouvé
  // par la CI du 12 septembre 2026 : un écart attendu sur l'accueil clair
  // (ligne des raccourcis plus longue) arrêtait le test avant le retour au
  // thème sombre ; la palette était alors capturée en clair (96 % d'écart),
  // restait ouverte faute d'Échap, et interceptait le clic du test suivant.
  // Une seule vraie différence en faisait quatre.
  it("accueil, thème clair", async () => {
    await theme("light");
    await browser.pause(200);
    const ecart = await browser.checkScreen("accueil-clair");
    await theme("dark");
    expect(ecart).toBeLessThanOrEqual(seuil);
  });

  it("palette ouverte", async () => {
    await browser.keys(["Control", "k"]);
    await $("#palette").waitForDisplayed({ timeout: 5000 });
    await browser.pause(200);
    const ecart = await browser.checkScreen("palette");
    await browser.keys("Escape");
    await $("#palette").waitForDisplayed({ reverse: true, timeout: 5000 });
    expect(ecart).toBeLessThanOrEqual(seuil);
  });

  it("modale « Connexion directe »", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.pause(200);
    const ecart = await browser.checkScreen("connexion-directe");
    await browser.keys("Escape");
    expect(ecart).toBeLessThanOrEqual(seuil);
  });
});
