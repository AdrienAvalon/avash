import { findHostRow, folderExists, openCtx } from "./helpers.js";

describe("Déplacer un hôte dans un dossier", () => {
  it("db-1 → « prod/db » via la modale « Déplacer vers… »", async () => {
    await openCtx(await findHostRow("db-1"));
    await browser.waitUntil(async () => (await $("#host-context").getAttribute("class")).includes("open"), { timeout: 5000, timeoutMsg: "menu hôte fermé" });
    await $('#host-context [data-act="move"]').click();
    await $("#move-modal").waitForDisplayed({ timeout: 5000 });
    await $("#move-new").setValue("prod/db");
    await $("#move-submit").click();
    // Le sous-dossier « db » apparaît (créé + hôte déplacé) et db-1 existe toujours.
    await browser.waitUntil(async () => folderExists("db"), { timeout: 8000, timeoutMsg: "sous-dossier db absent" });
    expect(await (await findHostRow("db-1")).isExisting()).toBe(true);
  });
});
