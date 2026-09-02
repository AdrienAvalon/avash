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
  // Un marqueur unique SANS deux caractères identiques consécutifs : la frappe
  // synthétique en perd un sur deux (« 1788 » arrivait « 178 », « asciicast »
  // « ascicast ») ; on intercale une lettre entre deux jumeaux.
  const marqueur = (base) => `${base}-${String(Date.now()).replace(/(.)(?=\1)/g, "$1x")}`;

  it("enregistre la sortie du terminal dans un fichier relisible", async () => {
    await (await findHostRow("test-ssh")).doubleClick();
    await browser.waitUntil(async () => (await $$(".state.live")).length > 0,
      { timeout: 20000, timeoutMsg: "session SSH jamais live" });

    // Premier enregistrement : il prouve que le shell traite bien ce qu'on tape
    // (les frappes envoyées pendant son démarrage ne le sont qu'après).
    await menu("record");
    await browser.waitUntil(async () => (await $$(".tab.rec")).length > 0,
      { timeout: 5000, timeoutMsg: "le voyant d'enregistrement n'est pas apparu" });
    const premierFichier = cheminDu(await dernierToast());
    expect(premierFichier).toBeDefined();
    const avant = marqueur("avant-du-lancement");
    await taper(`echo ${avant}\n`);
    await browser.waitUntil(() => readFileSync(premierFichier, "utf8").includes(avant),
      { timeout: 10000, timeoutMsg: "le premier marqueur n'est jamais arrivé" });
    await menu("record-stop");
    await browser.waitUntil(async () => (await $$(".tab.rec")).length === 0, { timeout: 5000 });

    // Second enregistrement, en cours de session : il doit s'ouvrir sur l'état
    // de l'écran — le marqueur y est déjà — et non sur du noir.
    await menu("record");
    await browser.waitUntil(async () => (await $$(".tab.rec")).length > 0, { timeout: 5000 });
    const chemin = cheminDu(await dernierToast());
    expect(chemin).toBeDefined();
    expect(chemin).not.toBe(premierFichier);
    const premier = JSON.parse(readFileSync(chemin, "utf8").split("\n")[1]);
    expect(premier[1]).toBe("o");
    expect(premier[2]).toContain(avant);

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
    expect(evenements.length).toBeGreaterThan(1);
    expect(evenements.every((e) => e[1] === "o" || e[1] === "r")).toBe(true);
    expect(evenements.some((e) => e[1] === "o" && e[2].includes("bonjour-cast-ok"))).toBe(true);

    // La liste des enregistrements, depuis la palette, montre les deux fichiers.
    await browser.keys(["Control", "k"]);
    const input = await $("#palette-input");
    await input.waitForDisplayed({ timeout: 5000 });
    await input.setValue("Enregistrements");
    const item = await $("#palette-results .item");
    await item.waitForDisplayed({ timeout: 5000 });
    await item.click();
    await browser.waitUntil(async () => (await $$("#enregistrements-liste .enregistrement-row")).length > 0,
      { timeout: 5000, timeoutMsg: "la liste des enregistrements est restée vide" });
    const listes = await browser.execute(() => [...document.querySelectorAll("#enregistrements-liste .enregistrement-row")].map((r) => r.dataset.chemin));
    expect(listes).toContain(chemin);
    expect(listes).toContain(premierFichier);
    expect(listes[0]).toBe(chemin, "le plus récent d'abord");
    await browser.keys("Escape");

    // Arrêté, l'enregistrement ne reprend pas : ce qui suit n'y entre pas. Le
    // marqueur est unique : le shell garde l'historique des passages précédents
    // et le proposerait en autosuggestion — donc dans la sortie — avant l'arrêt.
    const apres = marqueur("apres-stop");
    await taper(`echo ${apres}\n`);
    await browser.pause(800);
    expect(readFileSync(chemin, "utf8")).not.toContain(apres);
  });
});
