// Chaque fichier de spécifications doit partir d'un état propre : celui semé par
// le harnais, et rien d'autre. Sans cela la suite dépend de son ordre d'exécution
// — un fichier qui crée un dossier ou déplace un hôte fausse les suivants.
// Ce scénario est le garde-fou de cette propriété.

describe("Isolation entre fichiers de tests", () => {
  it("l'état de départ est celui semé, sans reste des autres scénarios", async () => {
    // Attendre que la liste soit peuplée avant de juger de son contenu.
    await browser.waitUntil(
      async () => (await $$("#host-list .host")).length > 0,
      { timeout: 8000, timeoutMsg: "la liste d'hôtes reste vide" },
    );

    const dossiers = await browser.execute(() =>
      [...document.querySelectorAll("#host-list .folder-row .fname")].map((e) => e.textContent),
    );
    const hotes = await browser.execute(() =>
      [...document.querySelectorAll("#host-list .host .alias")].map((e) => e.textContent),
    );

    // Le harnais ne sème qu'un seul dossier : « prod » (il contient web-1).
    expect(dossiers.sort()).toEqual(["prod"]);
    // Et trois hôtes : web-1, db-1 et l'hôte SSH réellement joignable.
    expect(hotes.sort()).toEqual(["db-1", "test-ssh", "web-1"]);
  });
});
