// Enregistrement d'une session au format asciicast (asciinema).
//
// Sur la session SSH réelle : on démarre l'enregistrement depuis le menu
// contextuel du terminal, on fait afficher un mot connu, on arrête, puis on
// relit le fichier annoncé — en-tête v2 et un événement de sortie portant le
// mot. Seule la sortie est enregistrée : le fichier ne doit contenir aucun
// événement de frappe.
import { readFileSync } from "node:fs";
import { findHostRow } from "./helpers.js";

describe("Enregistrement de session (asciicast)", () => {
  const menu = async (act) => {
    const zone = await $("#terminal");
    await browser.execute((el) => {
      el.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, clientX: 200, clientY: 200 }));
    }, zone);
    const item = await $(`#term-context [data-act="${act}"]`);
    await item.waitForDisplayed({ timeout: 5000, timeoutMsg: `entrée ${act} absente du menu` });
    await item.click();
  };
  const dernierToast = () => browser.execute(() => {
    const t = [...document.querySelectorAll("#toasts .toast")].pop();
    return t ? t.textContent : null;
  });

  const taper = async (texte) => {
    // xterm écoute sur son textarea caché : on le focalise avant de taper.
    // Pas de lettre doublée dans ce qu'on tape : une frappe synthétique répétée
    // trop vite est parfois perdue par le composant (« asciicast » arrivait
    // « ascicast »), et ce n'est pas ce que le scénario vérifie.
    await browser.execute(() => document.querySelector(".xterm-helper-textarea")?.focus());
    await browser.keys(texte);
  };
  const cheminDu = (toast) => /: (\/\S+\.cast)/.exec(toast ?? "")?.[1];

  it("enregistre la sortie du terminal dans un fichier relisible", async () => {
    await (await findHostRow("test-ssh")).doubleClick();
    await browser.waitUntil(async () => (await $$(".state.live")).length > 0,
      { timeout: 20000, timeoutMsg: "session SSH jamais live" });

    await menu("record");
    await browser.waitUntil(async () => (await $$(".tab.rec")).length > 0,
      { timeout: 5000, timeoutMsg: "le voyant d'enregistrement n'est pas apparu" });
    const chemin = cheminDu(await dernierToast());
    expect(chemin).toBeDefined();

    await taper("echo bonjour-cast-ok\n");
    // Le fichier est écrit au fil de l'eau : l'écho du serveur y arrive sans
    // attendre l'arrêt.
    await browser.waitUntil(() => readFileSync(chemin, "utf8").includes("bonjour-cast-ok"),
      { timeout: 10000, timeoutMsg: "l'écho n'est jamais arrivé dans l'enregistrement" });

    await menu("record-stop");
    await browser.waitUntil(async () => (await $$(".tab.rec")).length === 0, { timeout: 5000 });
    expect(cheminDu(await dernierToast())).toBe(chemin);

    const contenu = readFileSync(chemin, "utf8");
    const lignes = contenu.trim().split("\n");
    const entete = JSON.parse(lignes[0]);
    expect(entete.version).toBe(2);
    expect(entete.title).toBe("test-ssh");
    expect(entete.width).toBeGreaterThan(0);
    const evenements = lignes.slice(1).map((l) => JSON.parse(l));
    expect(evenements.length).toBeGreaterThan(0);
    expect(evenements.every((e) => e[1] === "o" || e[1] === "r")).toBe(true);
    expect(evenements.some((e) => e[1] === "o" && e[2].includes("bonjour-cast-ok"))).toBe(true);

    // Arrêté, l'enregistrement ne reprend pas : ce qui suit n'y entre pas. Le
    // marqueur est unique : le shell garde l'historique des passages précédents
    // et le proposerait en autosuggestion — donc dans la sortie — avant l'arrêt.
    const apres = `apres-stop-${Date.now()}`;
    await taper(`echo ${apres}\n`);
    await browser.pause(800);
    expect(readFileSync(chemin, "utf8")).not.toContain(apres);
  });
});
