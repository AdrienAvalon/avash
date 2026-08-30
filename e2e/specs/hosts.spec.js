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
  });
});
