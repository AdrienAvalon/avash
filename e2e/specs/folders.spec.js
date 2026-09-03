import { attendreDossier, findFolderRow, folderExists, openCtx } from "./helpers.js";

describe("Dossiers — cycle de vie complet", () => {
  it("« + dossier » ouvre askText (pas prompt) et crée le dossier", async () => {
    await $("#new-folder-btn").click();
    await $("#ask-modal").waitForDisplayed({ timeout: 5000 });
    await $("#ask-input").setValue("clients");
    await $("#ask-submit").click();
    await attendreDossier("clients");
  });

  it("crée un sous-dossier via le menu contextuel", async () => {
    await openCtx(await findFolderRow("clients"));
    await browser.waitUntil(async () => (await $("#folder-context").getAttribute("class")).includes("open"), { timeout: 5000, timeoutMsg: "menu dossier fermé" });
    await $('#folder-context [data-act="new"]').click();
    await $("#ask-modal").waitForDisplayed({ timeout: 5000 });
    await $("#ask-input").setValue("acme");
    await $("#ask-submit").click();
    await attendreDossier("acme");
  });

  it("renomme un dossier via le menu contextuel", async () => {
    await openCtx(await findFolderRow("acme"));
    await browser.waitUntil(async () => (await $("#folder-context").getAttribute("class")).includes("open"), { timeout: 5000 });
    await $('#folder-context [data-act="rename"]').click();
    await $("#ask-modal").waitForDisplayed({ timeout: 5000 });
    await $("#ask-input").setValue("acme-corp");
    await $("#ask-submit").click();
    await attendreDossier("acme-corp");
  });

  it("supprime un dossier via la modale de confirmation maison (bug WebKitGTK évité)", async () => {
    await openCtx(await findFolderRow("clients"));
    await browser.waitUntil(async () => (await $("#folder-context").getAttribute("class")).includes("open"), { timeout: 5000 });
    await $('#folder-context [data-act="delete"]').click();
    // La modale maison doit apparaître ET attendre le clic (confirm() natif ne bloque pas).
    await $("#confirm-modal").waitForDisplayed({ timeout: 5000 });
    await $("#confirm-ok").click();
    await browser.waitUntil(async () => !(await folderExists("clients")), { timeout: 8000, timeoutMsg: "clients pas supprimé" });
  });

  it("annuler la confirmation NE supprime PAS (le refus est respecté)", async () => {
    // Dossier dédié pour ne dépendre d'aucun test précédent.
    await $("#new-folder-btn").click();
    await $("#ask-modal").waitForDisplayed({ timeout: 5000 });
    await $("#ask-input").setValue("a-garder");
    await $("#ask-submit").click();
    await attendreDossier("a-garder");

    await openCtx(await findFolderRow("a-garder"));
    await browser.waitUntil(async () => (await $("#folder-context").getAttribute("class")).includes("open"), { timeout: 5000 });
    await $('#folder-context [data-act="delete"]').click();
    await $("#confirm-modal").waitForDisplayed({ timeout: 5000 });
    await $("#confirm-cancel").click();
    // Toujours là après annulation : le refus est bien respecté. On attend la
    // fermeture de la confirmation — état observable qui dit que le clic a été
    // traité — plutôt que de parier sur une durée.
    await $("#confirm-modal").waitForDisplayed({ reverse: true, timeout: 5000 });
    expect(await folderExists("a-garder")).toBe(true);
  });
});
