import { findHostRow, folderExists } from "./helpers.js";

describe("Hôtes SSH (config semée)", () => {
  it("affiche les hôtes semés et le dossier « prod »", async () => {
    await browser.waitUntil(
      async () => {
        try { await findHostRow("db-1"); return true; } catch { return false; }
      },
      { timeout: 8000, timeoutMsg: "db-1 n'apparaît pas" },
    );
    await expect(await folderExists("prod")).toBe(true); // web-1 y est rangé
  });

  it("un clic surligne l'hôte (.picked), un clic ailleurs déplace le surlignage", async () => {
    const db1 = await findHostRow("db-1");
    await db1.click();
    await expect((await db1.getAttribute("class")).includes("picked")).toBe(true);

    // Trouvé par l'audit du 8 septembre 2026 : le titre promet « un clic ailleurs
    // déplace le surlignage », mais le test s'arrêtait au premier clic et ne
    // vérifiait jamais le déplacement. Une régression du chemin CLIC (main.ts:195,
    // distinct du chemin focus/clavier de main.ts:100-102 couvert par
    // liste-clavier.spec.js) posant « picked » sans retirer l'ancien laissait deux
    // hôtes surlignés — symptôme déjà vu au clavier (liste-clavier.spec.js:59) —
    // sans faire rougir ce scénario. On clique une seconde ligne et l'on exige que
    // le surlignage ait bougé : un seul « picked », sur web-1, plus sur db-1.
    const web1 = await findHostRow("web-1");
    await web1.click();
    await browser.waitUntil(async () => (await $$("#host-list .picked")).length === 1, {
      timeout: 3000,
      timeoutMsg: "le surlignage ne s'est pas déplacé sur un seul hôte",
    });
    await expect((await (await findHostRow("web-1")).getAttribute("class")).includes("picked")).toBe(true);
    await expect((await (await findHostRow("db-1")).getAttribute("class")).includes("picked")).toBe(false);
  });
});
