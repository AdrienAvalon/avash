// Connexion SSH RÉELLE : double-clic sur l'hôte « test-ssh » semé, servi par le
// sshd local (auth par clé). Valide toute la chaîne UI → russh → PTY.
import { findHostRow } from "./helpers.js";

describe("SSH — connexion réelle (sshd local)", () => {
  it("double-clic sur test-ssh ouvre une session live", async () => {
    await (await findHostRow("test-ssh")).doubleClick();
    // .state.live => pty_open a réussi (connexion + auth par clé + shell ouvert).
    await browser.waitUntil(async () => (await $$(".state.live")).length > 0,
      { timeout: 20000, timeoutMsg: "session SSH jamais live" });
    // L'accueil s'efface au profit du terminal.
    await browser.waitUntil(async () => !(await $("#terminal-empty").isDisplayed()),
      { timeout: 5000, timeoutMsg: "accueil encore visible" });
  });
});
