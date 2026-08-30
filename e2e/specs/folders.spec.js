// Aurait attrapé le bug critique « window.prompt() inopérant sous WebKitGTK » :
// sans la modale askText, aucun dialogue n'apparaît et le test échoue d'emblée.
describe("Dossiers", () => {
  it("« + dossier » ouvre la modale askText et crée le dossier dans l'arbre", async () => {
    await $("#new-folder-btn").click();
    await $("#ask-modal").waitForDisplayed({ timeout: 5000 });
    await $("#ask-input").setValue("prod-e2e");
    await $("#ask-submit").click();
    // Le dossier apparaît dans l'arbre (rendu asynchrone). textContent plutôt que
    // getText() : ce dernier renvoie vide via l'automation WebKitGTK.
    await browser.waitUntil(
      async () => {
        const rows = await $$("#host-list .folder-row .fname");
        for (const r of rows) if ((await r.getProperty("textContent")) === "prod-e2e") return true;
        return false;
      },
      { timeout: 8000, timeoutMsg: "le dossier « prod-e2e » n'apparaît pas dans l'arbre" },
    );
  });
});
