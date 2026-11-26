// La fenêtre des paramètres, et Ctrl+C / Ctrl+V « comme Windows Terminal »,
// sur la vraie application et une session SSH réelle. Demandés par le
// mainteneur le 26 novembre 2026 : « un vrai bouton pour les paramètres
// plutôt que de devoir taper Ctrl+K », et « Ctrl+C pour copier et Ctrl+V pour
// coller ne fonctionnent pas en SSH sous Windows ».
//
// L'option est active d'office sous Windows seulement : ici on l'active par
// la fenêtre, ce qui exerce aussi la case. Sous Windows, elle l'est déjà, et
// le scénario la laisse cochée.
import { doubleCliquerHote, ecouterSortiePty, attendreSessionLive, openCtx, resultatDeEcho, attendreShellCalme, sortieSansSequences, ecrirePressePapiers, lirePressePapiers, taperDansLeTerminal } from "./helpers.js";
import { EMBARQUE } from "../wdio.conf.js";

const JETON = "pobiruxe";

describe("Paramètres, et Ctrl+C / Ctrl+V dans un terminal SSH", () => {
  it("la roue crantée ouvre les paramètres, et la case active Ctrl+C / Ctrl+V", async () => {
    await $("#parametres-btn").click();
    await $("#parametres-modal").waitForDisplayed({ timeout: 5000 });
    expect(await $("#parametres-modal-title").getText()).toBe("Paramètres");
    const option = await $("#param-ctrl");
    if (!(await option.isSelected())) await option.click();
    expect(await option.isSelected()).toBe(true);
    await $("#parametres-close").click();
    await $("#parametres-modal").waitForDisplayed({ reverse: true, timeout: 5000 });
  });

  it("Ctrl+V colle une seule fois", async () => {
    await ecouterSortiePty();
    await doubleCliquerHote("test-ssh");
    await attendreSessionLive();
    await attendreShellCalme();
    await ecrirePressePapiers(JETON);
    await taperDansLeTerminal("echo C");
    await browser.keys(["Control", "v"]);
    await browser.pause(800);
    expect(await resultatDeEcho("C", JETON)).toBe(JETON);
  });

  it("Ctrl+C sans sélection interrompt toujours la commande", async () => {
    // Le shell a fini de répondre à la commande précédente : tapé pendant
    // qu'il redessinait sa ligne, le texte se perdait (serveur embarqué).
    await attendreShellCalme();
    // `cat` attend sur son entrée jusqu'à l'interruption : sans elle, l'echo
    // qui suit lui serait donné, pas au shell. (Pas de lettre doublée : la
    // frappe simulée en avale une, « sleep » devenait « slep ».)
    await taperDansLeTerminal("cat");
    await browser.keys(["Enter"]);
    await browser.pause(700);
    await browser.keys(["Control", "c"]);
    await browser.pause(300);
    // Sans sélection, rien n'a été copié : le presse-papiers tient toujours
    // le texte collé au scénario précédent.
    expect(await lirePressePapiers()).toBe(JETON);
    // Le serveur WebDriver embarqué (Windows, macOS) ne fait parvenir aucun
    // caractère de contrôle au shell, quelle que soit l'application (voir
    // « Ctrl+B atteint le shell distant », ssh.spec.js) : l'interruption
    // elle-même ne s'y vérifie pas.
    if (EMBARQUE) return;
    await taperDansLeTerminal("echo coupure-ok");
    await browser.keys(["Enter"]);
    await browser.waitUntil(async () => /^coupure-ok$/m.test(await sortieSansSequences()),
      { timeout: 6000, timeoutMsg: "Ctrl+C n'a pas interrompu cat" });
  });

  it("Ctrl+C avec une sélection la copie", async () => {
    // Sous le serveur embarqué, cat tourne encore : son écho suffit à
    // donner au terminal un texte reconnaissable.
    if (EMBARQUE) {
      await taperDansLeTerminal("coupure-ok");
      await browser.keys(["Enter"]);
      await browser.pause(500);
    }
    await openCtx(await $(".xterm"));
    await $('#term-context [data-act="selectall"]').click();
    await browser.keys(["Control", "c"]);
    await browser.waitUntil(async () => String(await lirePressePapiers()).includes("coupure-ok"),
      { timeout: 5000, timeoutMsg: "Ctrl+C n'a pas copié la sélection du terminal" });
  });
});
