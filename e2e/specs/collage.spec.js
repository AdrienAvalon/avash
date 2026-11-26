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
// Le presse-papiers est rempli par le plugin de Tauri (`ecrirePressePapiers`),
// le texte tapé par `taperDansLeTerminal` : sous Windows et macOS, le serveur
// WebDriver embarqué ne synthétise que des événements, et doublait chaque
// caractère d'un `browser.keys` (« eecchhoo », vu en CI le 26 novembre 2026).
// Pas de lettre doublée dans le jeton : la frappe de tauri-driver en avalait
// une.
import { doubleCliquerHote, ecouterSortiePty, attendreSessionLive, openCtx, resultatDeEcho, attendreShellCalme, ecrirePressePapiers, taperDansLeTerminal } from "./helpers.js";

const JETON = "kwazomeb";

async function focusDansLeTerminal() {
  return browser.execute(() => document.activeElement?.classList.contains("xterm-helper-textarea") === true);
}

describe("Copier-coller dans un terminal SSH", () => {
  before(async () => {
    await ecouterSortiePty();
    await doubleCliquerHote("test-ssh");
    await attendreSessionLive();
    await attendreShellCalme();
    await ecrirePressePapiers(JETON);
  });

  it("Ctrl+Maj+V colle une seule fois, et le terminal garde le focus", async () => {
    await $(".xterm textarea").click();
    await taperDansLeTerminal("echo A");
    await browser.keys(["Control", "Shift", "v"]);
    await browser.pause(800);
    expect(await focusDansLeTerminal()).toBe(true);
    expect(await resultatDeEcho("A", JETON)).toBe(JETON);
  });

  it("« Coller » du menu colle une seule fois, et rend le focus au terminal", async () => {
    await taperDansLeTerminal("echo B");
    await openCtx(await $(".xterm"));
    await $('#term-context [data-act="paste"]').click();
    await browser.waitUntil(focusDansLeTerminal,
      { timeout: 3000, timeoutMsg: "le terminal n'a pas retrouvé le focus après « Coller »" });
    await browser.pause(800);
    expect(await resultatDeEcho("B", JETON)).toBe(JETON);
  });
});
