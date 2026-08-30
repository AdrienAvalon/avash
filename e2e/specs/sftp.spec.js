// Panneau SFTP sur une session SSH réelle (sshd local) : ouvre le panneau et
// vérifie que le répertoire distant se liste. Valide UI → sftp_list → russh/SFTP.
import { findHostRow } from "./helpers.js";

describe("SFTP — lister le répertoire distant", () => {
  it("ouvre le panneau et affiche des entrées", async () => {
    await (await findHostRow("test-ssh")).doubleClick();
    await browser.waitUntil(async () => (await $$(".state.live")).length > 0,
      { timeout: 20000, timeoutMsg: "session SSH jamais live" });

    // Le bouton SFTP s'active une fois la session ouverte.
    await browser.waitUntil(async () => $("#sftp-toggle").isEnabled(),
      { timeout: 5000, timeoutMsg: "bouton SFTP jamais activé" });
    await $("#sftp-toggle").click();
    await $("#sftp-panel").waitForDisplayed({ timeout: 5000 });

    // Le listing distant (home de l'utilisateur du sshd) se remplit d'entrées.
    await browser.waitUntil(async () => (await $$("#sftp-list .sftp-entry")).length > 0,
      { timeout: 15000, timeoutMsg: "aucune entrée SFTP listée" });
  });
});
