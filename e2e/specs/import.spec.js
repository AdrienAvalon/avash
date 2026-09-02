// Import de sessions PuTTY.
//
// Le harnais sème trois fichiers dans ~/.putty/sessions : une vraie session
// SSH, « Default Settings » (à ignorer) et une session série (à compter, pas
// à reprendre). L'import doit proposer la bonne, dire ce qu'il ne reprend pas
// (la clé .ppk), écrire l'hôte, puis le reconnaître comme doublon.

describe("Import de sessions", () => {
  const lignes = () => $$("#import-list [data-alias]");
  // Sous Windows, les sessions PuTTY se lisent dans le registre, pas dans les
  // fichiers semés ici : le scénario n'y a pas de matière.
  before(function () { if (process.platform === "win32") this.skip(); });

  it("propose la session PuTTY, avec sa remarque, et ignore le reste", async () => {
    await $("#import-btn").click();
    await browser.waitUntil(async () => (await lignes()).length > 0, {
      timeout: 10000, timeoutMsg: "aucune session proposée",
    });
    const alias = await browser.execute(() => [...document.querySelectorAll("#import-list [data-alias]")].map((e) => e.dataset.alias));
    expect(alias).toEqual(["prod-web"]);
    const texte = await browser.execute(() => document.querySelector("#import-list").textContent);
    expect(texte).toContain("adrien@10.0.0.7:2222");
    expect(texte).toContain(".ppk");
    const bilan = await browser.execute(() => document.querySelector("#import-bilan").textContent);
    expect(bilan).toContain("1 session d'un autre protocole");
  });

  it("écrit l'hôte, qui apparaît dans la liste", async () => {
    await $("#import-submit").click();
    await browser.waitUntil(
      async () => (await browser.execute(() => [...document.querySelectorAll("#host-list [data-cle]")].map((e) => e.dataset.cle))).some((c) => c.endsWith("prod-web")),
      { timeout: 10000, timeoutMsg: "l'hôte importé n'apparaît pas" },
    );
    const ouverte = await browser.execute(() => document.querySelector("#import-modal").classList.contains("open"));
    expect(ouverte).toBe(false);
  });

  it("reconnaît ensuite le même serveur comme doublon, décoché", async () => {
    await $("#import-btn").click();
    await browser.waitUntil(async () => (await lignes()).length > 0, { timeout: 10000 });
    const etat = await browser.execute(() => {
      const l = document.querySelector("#import-list [data-alias]");
      return { alias: l.dataset.alias, coche: l.querySelector("input[type=checkbox]").checked, texte: l.textContent };
    });
    expect(etat.alias).toBe("prod-web-2");
    expect(etat.coche).toBe(false);
    expect(etat.texte).toContain("Déjà déclaré sous « prod-web »");
    await browser.keys(["Escape"]);
  });
});
