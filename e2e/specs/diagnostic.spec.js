// « Exporter un diagnostic… » : l'entrée est dans la palette.
//
// Contrat K13 (audit du 12 septembre 2026, C-ipc-3) : `diagnostic_exporter` ne
// prend plus de chemin venu de la page, il ouvre lui-même la boîte native
// « Enregistrer sous », qu'aucun pilote ne sait conduire. Ce scénario appelait
// la commande avec un chemin du bac à sable pour relire le fichier ; ce qui
// s'y vérifiait (en-tête, configuration comptée et non recopiée, aucun mot de
// passe) l'est désormais par les tests de `ecrire_diagnostic` (voie interface,
// commands/diagnostic.rs), et le front par barre-laterale-reconciliee.dom.test.ts
// (la page n'envoie plus de chemin). Il reste ici l'entrée de palette.
describe("Diagnostic — export pour un ticket", () => {
  it("la palette propose l'export du diagnostic", async () => {
    await browser.keys(["Control", "k"]);
    const input = await $("#palette-input");
    await input.waitForDisplayed({ timeout: 5000 });
    await input.setValue("diagnostic");
    await browser.waitUntil(async () => {
      // `$$().map` est déjà asynchrone chez WebdriverIO : pas de Promise.all.
      const textes = await $$("#palette-results .item .name").map((e) => e.getProperty("textContent"));
      return textes.some((x) => String(x).includes("Exporter un diagnostic"));
    }, { timeout: 5000, timeoutMsg: "l'entrée « Exporter un diagnostic… » n'est pas dans la palette" });
    await browser.keys("Escape");
  });
});
