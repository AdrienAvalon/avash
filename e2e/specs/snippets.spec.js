import { trouverLigne } from "./helpers.js";

describe("Snippets — créer puis supprimer", () => {
  it("crée un snippet, le voit dans la liste, le supprime (askConfirm)", async () => {
    await $("#snippets-btn").click();
    await $("#snippets-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => document.getElementById("snippet-block").setAttribute("open", ""));
    await $("#sn-name").setValue("Snippet E2E");
    await $("#sn-command").setValue("echo bonjour");
    await $("#sn-submit").click();

    // Apparaît dans la liste.
    // `trouverLigne` tolère une reconstruction de la liste pendant le parcours :
    // une référence caduque levait une erreur qui avortait le `waitUntil`.
    const findRow = () => trouverLigne("#snippet-list .snippet-row", ".snm", "Snippet E2E");
    await browser.waitUntil(async () => (await findRow()) !== null, { timeout: 8000, timeoutMsg: "snippet non listé" });

    // Supprime -> askConfirm -> confirmer.
    await (await findRow()).$('[data-act="delete"]').click();
    await $("#confirm-modal").waitForDisplayed({ timeout: 5000 });
    await $("#confirm-ok").click();
    await browser.waitUntil(async () => (await findRow()) === null, { timeout: 8000, timeoutMsg: "snippet pas supprimé" });
  });
});
