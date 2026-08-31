import { trouverLigne } from "./helpers.js";

describe("Tunnels — créer puis supprimer une définition", () => {
  it("crée un tunnel local, le voit dans la liste, le supprime (askConfirm)", async () => {
    await $("#tunnels-btn").click();
    await $("#tunnels-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => document.getElementById("tunnel-block").setAttribute("open", ""));
    await $("#t-alias").selectByIndex(0);       // un hôte semé (web-1/db-1)
    await $("#t-bind").setValue("18080");
    await $("#t-host").setValue("localhost");
    await $("#t-port").setValue("5432");
    await $("#t-name").setValue("Tunnel E2E");
    await $("#t-submit").click();

    // `trouverLigne` tolère une reconstruction de la liste pendant le parcours :
    // une référence caduque levait une erreur qui avortait le `waitUntil`.
    const findRow = () => trouverLigne("#tunnel-list .tunnel-row", ".tname", "Tunnel E2E");
    await browser.waitUntil(async () => (await findRow()) !== null, { timeout: 8000, timeoutMsg: "tunnel non listé" });

    await (await findRow()).$('[data-act="delete"]').click();
    await $("#confirm-modal").waitForDisplayed({ timeout: 5000 });
    await $("#confirm-ok").click();
    await browser.waitUntil(async () => (await findRow()) === null, { timeout: 8000, timeoutMsg: "tunnel pas supprimé" });
  });
});
