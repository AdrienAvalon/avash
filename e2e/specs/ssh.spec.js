// Connexion SSH RÉELLE : double-clic sur l'hôte « test-ssh » semé, servi par le
// sshd local (auth par clé). Valide toute la chaîne UI → russh → PTY.
import { attendreSessionLive, doubleCliquerHote, ecouterSortiePty, fermerOngletActif, sortiePty, taperDansLeTerminal } from "./helpers.js";

describe("SSH — connexion réelle (sshd local)", () => {
  it("double-clic sur test-ssh ouvre une session live", async () => {
    await ecouterSortiePty();
    await doubleCliquerHote("test-ssh");
    // .state.live => pty_open a réussi (connexion + auth par clé + shell ouvert).
    await attendreSessionLive();
    // L'accueil s'efface au profit du terminal.
    await browser.waitUntil(async () => !(await $("#terminal-empty").isDisplayed()),
      { timeout: 5000, timeoutMsg: "accueil encore visible" });
  });

  // Audit du 12 septembre 2026 (C-front-2) : xterm voyait Ctrl+Tab avant
  // l'application et l'envoyait au shell en tabulation, qui complétait « ec » en
  // « echo ». Un seul onglet ici : le raccourci n'a rien à faire, le shell ne
  // doit rien recevoir. « ec » n'étant pas une commande, le shell le dit (bash,
  // dash, zsh et fish l'écrivent chacun à leur façon).
  it("Ctrl+Tab ne tabule pas dans le shell", async () => {
    await taperDansLeTerminal("ec");
    await browser.keys(["Control", "Tab"]);
    await browser.keys("Enter");
    const introuvable = /ec: (command )?not found|command not found: ec|unknown command: ec/i;
    await browser.waitUntil(async () => introuvable.test(await sortiePty()), {
      timeout: 10000,
      timeoutMsg: "« ec » a été complété : Ctrl+Tab est parti au shell",
    });
  });

  // Audit du 12 septembre 2026 (C-front-3) : Ctrl+B, préfixe de tmux, était
  // confisqué par le panneau SFTP même terminal focalisé. Il va désormais au
  // shell, où readline, zsh et fish le lient à « un caractère en arrière » :
  // « echo mk », Ctrl+B, « zv » affiche « mzvk ». Confisqué, il afficherait
  // « mkzv ». Le panneau SFTP ne s'ouvre pas.
  it("Ctrl+B atteint le shell distant", async () => {
    await taperDansLeTerminal("echo mk");
    await browser.keys(["Control", "b"]);
    await taperDansLeTerminal("zv");
    await browser.keys("Enter");
    await browser.waitUntil(async () => (await sortiePty()).includes("mzvk"), {
      timeout: 10000,
      timeoutMsg: "Ctrl+B n'a pas atteint le shell distant",
    });
    expect(await $("#sftp-panel").isDisplayed()).toBe(false);
  });

  // Audit du 12 septembre 2026 (C-front-6) : fermer une session vivante ne
  // demandait rien. Ctrl+W par réflexe readline, ou la croix visée de travers,
  // tuait ce qui tournait dedans.
  it("fermer un onglet vivant demande confirmation", async () => {
    await browser.keys(["Control", "w"]);
    const modale = await $("#confirm-modal");
    await modale.waitForDisplayed({ timeout: 5000, timeoutMsg: "Ctrl+W a fermé une session vivante sans rien demander" });
    await $("#confirm-cancel").click();
    await modale.waitForDisplayed({ reverse: true, timeout: 5000 });
    // Renoncer garde la session, vivante.
    expect((await $$(".tab")).length).toBe(1);
    expect((await $$(".state.live")).length).toBe(1);
    // La croix aussi demande ; confirmer ferme.
    await fermerOngletActif({ vivant: true });
    await browser.waitUntil(async () => (await $$(".tab")).length === 0, {
      timeout: 5000,
      timeoutMsg: "l'onglet confirmé ne s'est pas fermé",
    });
  });
});
