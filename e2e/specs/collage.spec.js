// Copier-coller dans une session SSH réelle. Signalé par le mainteneur le
// 26 novembre 2026 : « parfois ça copie en double en SSH et quand on a fait
// coller on perd le focus du terminal ».
//
// - Ctrl+Maj+V lisait le presse-papiers ET laissait le navigateur faire son
//   collage natif (WebView2 : « coller en texte brut »), que l'interception
//   collait une seconde fois. Sous WebKitGTK, la lecture était refusée et seul
//   le collage natif passait. Désormais : action par défaut empêchée, lecture
//   par le plugin de Tauri, un seul collage partout.
// - « Coller » du menu du clic droit lisait par navigator.clipboard, refusé
//   sous WebKitGTK : rien n'était collé sous Linux. Et le clic sur l'entrée du
//   menu ôtait le focus à la zone de saisie du terminal.
//
// Le texte copié l'est par un vrai geste (saisi dans le filtre des hôtes,
// sélectionné, Ctrl+C) : sans geste, le moteur refuse d'écrire dans le
// presse-papiers. Pas de lettre doublée dans le jeton : la frappe simulée en
// avalait une.
import { doubleCliquerHote, ecouterSortiePty, attendreSessionLive, sortiePty, openCtx } from "./helpers.js";

const JETON = "kwazomeb";

/** Ce que le shell a reçu, lu dans le résultat d'un `echo <préfixe><collage>` :
 *  compter le jeton dans la sortie brute ne dit rien, fish redessinant sa ligne
 *  de commande à chaque modification. Rend le texte collé après le préfixe. */
async function resultatDeEcho(prefixe) {
  await browser.keys(["Enter"]);
  const motif = new RegExp(`^${prefixe}((?:${JETON})+)$`, "m");
  let trouve = null;
  try {
    await browser.waitUntil(async () => {
      trouve = (await sortieSansSequences()).match(motif);
      return trouve !== null;
    }, { timeout: 5000 });
  } catch (e) {
    throw new Error(`aucun résultat de « echo ${prefixe}… » ; fin du terminal :\n${(await sortiePty()).slice(-600)}`, { cause: e });
  }
  return trouve[1];
}

/** La sortie du terminal sans aucune séquence d'échappement : `sortiePty` ne
 *  retire que les CSI, et fish fait précéder le résultat d'une commande de
 *  séquences OSC (titre, marqueurs 133). */
async function sortieSansSequences() {
  const brut = String(await browser.execute(() => window.__sortiePty ?? ""));
  return brut
    // eslint-disable-next-line no-control-regex
    .replace(/\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)/g, "")
    // eslint-disable-next-line no-control-regex
    .replace(/\x1b\[[0-9;?>=]*[A-Za-z~]/g, "")
    // eslint-disable-next-line no-control-regex
    .replace(/\x1b[=>]/g, "")
    .replace(/\r/g, "");
}

async function focusDansLeTerminal() {
  return browser.execute(() => document.activeElement?.classList.contains("xterm-helper-textarea") === true);
}

describe("Copier-coller dans un terminal SSH", () => {
  before(async () => {
    await ecouterSortiePty();
    await doubleCliquerHote("test-ssh");
    await attendreSessionLive();
    // L'invite du shell : plus rien n'arrive depuis un moment.
    let avant = -1;
    await browser.waitUntil(async () => {
      const n = await browser.execute(() => (window.__sortiePty ?? "").length);
      const calme = n > 0 && n === avant;
      avant = n;
      return calme;
    }, { timeout: 15000, interval: 700, timeoutMsg: "le shell n'a jamais fini de démarrer" });
    await $("#search").click();
    await browser.keys(JETON);
    await browser.keys(["Control", "a"]);
    await browser.keys(["Control", "c"]);
    await browser.keys(["Backspace"]);
  });

  it("Ctrl+Maj+V colle une seule fois, et le terminal garde le focus", async () => {
    await $(".xterm textarea").click();
    await browser.keys("echo A");
    await browser.keys(["Control", "Shift", "v"]);
    await browser.pause(800);
    expect(await focusDansLeTerminal()).toBe(true);
    expect(await resultatDeEcho("A")).toBe(JETON);
  });

  it("« Coller » du menu colle une seule fois, et rend le focus au terminal", async () => {
    await browser.keys("echo B");
    await openCtx(await $(".xterm"));
    await $('#term-context [data-act="paste"]').click();
    await browser.waitUntil(focusDansLeTerminal,
      { timeout: 3000, timeoutMsg: "le terminal n'a pas retrouvé le focus après « Coller »" });
    await browser.pause(800);
    expect(await resultatDeEcho("B")).toBe(JETON);
  });
});
