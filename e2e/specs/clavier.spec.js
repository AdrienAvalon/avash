// Le clavier doit mener jusqu'à l'action principale : ouvrir une session.
// L'audit relevait que la palette exigeait la souris et que les modales
// empilées se fermaient à deux.
describe("Navigation au clavier", () => {
  // Les raccourcis passent par un écouteur sur `document` : il faut que le
  // focus soit dans la page, ce qui n'est pas acquis au tout premier instant.
  beforeEach(async () => {
    await $("#host-list").waitForExist({ timeout: 8000 });
    await $("#terminal-empty").click().catch(() => {});
  });

  it("la palette se parcourt aux flèches et Entrée valide la commande surlignée", async () => {
    await browser.keys(["Control", "k"]);
    await $("#palette").waitForDisplayed({ timeout: 5000 });
    await browser.waitUntil(
      async () => (await $$("#palette-results .item")).length > 0,
      { timeout: 5000, timeoutMsg: "la palette ne propose rien" },
    );
    // La première ligne est surlignée d'entrée de jeu.
    expect(await $("#palette-results .item.hl").isExisting()).toBe(true);
    await browser.keys("ArrowDown");
    const surligne = await browser.execute(() =>
      document.querySelector("#palette-results .item.hl")?.id,
    );
    expect(surligne).toBe("palette-item-1");
    await browser.keys("Escape");
    await browser.waitUntil(
      async () => !(await $("#palette").isDisplayed()),
      { timeout: 5000 },
    );

    // Trouvé par l'audit du 7 septembre 2026 : ce scénario annonçait « Entrée
    // valide » mais ne pressait que Échap, et vérifiait la seule fermeture ;
    // la branche `Enter` du keydown de #palette-input (main.ts, `choisie.ouvrir()`)
    // n'était couverte par aucun test, si bien qu'on pouvait l'ôter sans rien
    // faire rougir. Sans serveur, on vise la commande « Switch to English » :
    // la filtrer la remonte en tête (surlignée), Entrée la valide, l'interface
    // passe en anglais.
    const texte = (sel) => browser.execute((s) => document.querySelector(s)?.textContent?.trim() ?? null, sel);
    await browser.keys(["Control", "k"]);
    const input = await $("#palette-input");
    await input.waitForDisplayed({ timeout: 5000 });
    await input.setValue("Switch to English");
    await $("#palette-results .item.hl").waitForExist({ timeout: 5000 });
    await browser.keys("Enter");
    await browser.waitUntil(async () => (await texte("#manual-btn")) === "Direct connection", {
      timeout: 5000, timeoutMsg: "Entrée n'a pas validé la commande de langue surlignée",
    });

    // Revenir au français par la même voie : les autres scénarios affirment des
    // textes français, et le choix se mémorise (localStorage) au-delà du test.
    await browser.keys(["Control", "k"]);
    const retour = await $("#palette-input");
    await retour.waitForDisplayed({ timeout: 5000 });
    await retour.setValue("Passer en français");
    await $("#palette-results .item.hl").waitForExist({ timeout: 5000 });
    await browser.keys("Enter");
    await browser.waitUntil(async () => (await texte("#manual-btn")) === "Connexion directe", {
      timeout: 5000, timeoutMsg: "le retour au français n'a pas pris",
    });
  });

  it("Ctrl+K ne s'ouvre pas par-dessus une boîte de dialogue", async () => {
    // Sinon la palette volait le focus à la demande de mot de passe, et la
    // frappe suivante — le mot de passe — partait en clair dans sa recherche.
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.keys(["Control", "k"]);
    // « La palette ne doit PAS s'ouvrir » : on laisse le moteur traiter la
    // frappe, puis on constate — sans parier sur une durée.
    await browser.execute(() => new Promise((r) => requestAnimationFrame(() => r(null))));
    expect(await $("#palette").isDisplayed()).toBe(false);
    await browser.keys("Escape");
  });

  it("Échap ne ferme qu'une boîte à la fois", async () => {
    await $("#tunnels-btn").click();
    await $("#tunnels-modal").waitForDisplayed({ timeout: 8000 });
    // Une confirmation par-dessus : l'annuler ne doit pas emporter la fenêtre.
    await browser.execute(() => document.getElementById("confirm-modal").classList.add("open"));
    await browser.keys("Escape");
    // La confirmation, elle, doit s'être refermée : c'est l'état observable qui
    // dit que la touche a été traitée. La fenêtre du dessous doit rester.
    await $("#confirm-modal").waitForDisplayed({ reverse: true, timeout: 5000 });
    expect(await $("#tunnels-modal").isDisplayed()).toBe(true);
    await browser.execute(() => document.getElementById("confirm-modal").classList.remove("open"));
    await browser.keys("Escape");
  });
});
