// Mémoire des onglets : une session ouverte sur le sshd local, puis le front
// rechargé (ce que fait un relancement de l'application) ; l'accueil propose
// de rouvrir l'onglet, et « Rouvrir » remet une session live. « Ignorer »
// efface la mémoire : au rechargement suivant, plus rien n'est proposé.
import { doubleCliquerHote } from "./helpers.js";
import { EMBARQUE } from "../wdio.conf.js";

async function attendreDemarrage() {
  await browser.waitUntil(async () => browser.execute(() => {
    const l = document.getElementById("host-list");
    return document.readyState === "complete" && !!l && l.children.length > 0;
  }), { timeout: 30000, timeoutMsg: "l'application n'a pas redémarré" });
}

describe("Onglets — mémoire et réouverture", () => {
  before(function () {
    // Le scénario recharge la page pour simuler un relancement. Par le serveur
    // WebDriver embarqué (Windows, macOS), le rechargement coupe la session de
    // pilotage : la page ne répond plus (ECONNRESET sur `execute`, cinquième
    // passage Windows du 05/09/2026). Il reste joué sous Linux, par tauri-driver.
    if (EMBARQUE) this.skip();
  });

  it("propose de rouvrir l'onglet de la dernière fois, et le rouvre", async () => {
    await doubleCliquerHote("test-ssh");
    await browser.waitUntil(async () => (await $$(".state.live")).length > 0,
      { timeout: 20000, timeoutMsg: "session SSH jamais live" });
    // La mémoire s'écrit à l'ouverture : on n'attend pas une fermeture propre.
    await browser.waitUntil(async () => {
      const m = await browser.execute(() => window.__TAURI_INTERNALS__.invoke("onglets_memorises"));
      return Array.isArray(m) && m.length === 1 && m[0].alias === "test-ssh";
    }, { timeout: 5000, timeoutMsg: "la mémoire des onglets n'a pas été écrite" });

    await browser.execute(() => window.location.reload());
    await attendreDemarrage();
    const bandeau = await $("#restaurer");
    await bandeau.waitForDisplayed({ timeout: 10000 });
    expect(await $("#restaurer-texte").getText()).toContain("1 onglet");
    await $("#restaurer-ok").click();
    await browser.waitUntil(async () => (await $$(".state.live")).length > 0,
      { timeout: 20000, timeoutMsg: "l'onglet rouvert n'est jamais live" });
    expect(await $("#restaurer").isDisplayed()).toBe(false);
  });

  it("« Ignorer » oublie la mémoire", async () => {
    await browser.execute(() => window.location.reload());
    await attendreDemarrage();
    await $("#restaurer").waitForDisplayed({ timeout: 10000 });
    await $("#restaurer-non").click();
    await browser.waitUntil(async () => {
      const m = await browser.execute(() => window.__TAURI_INTERNALS__.invoke("onglets_memorises"));
      return Array.isArray(m) && m.length === 0;
    }, { timeout: 5000, timeoutMsg: "la mémoire n'a pas été effacée" });
    await browser.execute(() => window.location.reload());
    await attendreDemarrage();
    // Rien à proposer : le bandeau reste caché après le rendu des hôtes.
    await browser.pause(800);
    expect(await $("#restaurer").isDisplayed()).toBe(false);
  });
});
