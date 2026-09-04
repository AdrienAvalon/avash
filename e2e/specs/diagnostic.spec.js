// « Exporter un diagnostic… » : l'entrée est dans la palette, et la commande
// qu'elle appelle écrit un fichier lisible, sans mot de passe ni nom d'hôte
// de la configuration. La boîte d'enregistrement est native (pas pilotable) :
// le scénario vérifie l'entrée de palette, puis appelle la commande avec un
// chemin du bac à sable, comme le front le fait une fois le chemin choisi.
import { readFileSync, existsSync } from "node:fs";
import { join } from "node:path";

describe("Diagnostic — export pour un ticket", () => {
  it("la palette propose l'export et la commande écrit un fichier sans secret", async () => {
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

    const chemin = join(process.env.AVASH_E2E_SANDBOX, "diagnostic-e2e.txt");
    const rendu = await browser.execute(
      (c) => window.__TAURI_INTERNALS__.invoke("diagnostic_exporter", { chemin: c }),
      chemin,
    );
    expect(rendu).toBe(chemin);
    expect(existsSync(chemin)).toBe(true);
    const texte = readFileSync(chemin, "utf8");
    expect(texte.startsWith("# Diagnostic Avash ")).toBe(true);
    expect(texte).toContain("## Configuration");
    // La configuration semée est comptée, pas recopiée : ni alias ni adresse.
    expect(texte).toContain("hôte(s)");
    expect(texte).not.toContain("web-1");
    expect(texte).not.toContain("10.0.0.1");
    expect(texte.toLowerCase()).not.toContain("password");
  });
});
