// Port série : un pseudo-terminal tenu par socat, qui renvoie tout ce qu'il
// reçoit (EXEC:cat). La connexion directe ouvre ce port, on tape une commande,
// et l'écho revient par le même chemin que la sortie d'un shell SSH
// (événement `pty-output`), que le scénario écoute comme le front. Linux
// seulement : ni socat ni pseudo-terminal série sur les autres exécuteurs.
import { spawn } from "node:child_process";
import { existsSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { attendreSessionLive, ecouterSortiePty, sortiePty } from "./helpers.js";

const LINUX = process.platform === "linux";

describe("Port série — connexion directe sur un pseudo-terminal", () => {
  let socat;
  const dossier = LINUX ? mkdtempSync(join(tmpdir(), "avash-serie-")) : "";
  const port = join(dossier, "ttyA");

  before(async function () {
    if (!LINUX) this.skip();
    socat = spawn("socat", [`PTY,link=${port},raw,echo=0`, "EXEC:cat"], { stdio: "ignore" });
    await browser.waitUntil(() => existsSync(port), { timeout: 5000, timeoutMsg: "socat n'a pas créé le pseudo-terminal" });
  });

  after(() => { socat?.kill(); });

  it("ouvre le port, envoie une commande et reçoit son écho", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => {
      const r = document.querySelector('input[name="proto"][value="serie"]');
      r.checked = true;
      r.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await browser.waitUntil(async () => !(await $("#m-serie-row").getAttribute("hidden")) && (await $("#m-addr-row").getAttribute("hidden")) !== null,
      { timeout: 3000, timeoutMsg: "le formulaire n'est pas passé en mode série" });
    await $("#m-serie-chemin").setValue(port);
    await browser.execute(() => { document.getElementById("m-serie-vitesse").value = "9600"; });

    // On écoute la sortie AVANT de se connecter : ce que le port renvoie
    // arrive par `pty-output`, comme pour un shell SSH.
    await ecouterSortiePty();
    await $("#m-submit").click();
    await attendreSessionLive("la session série");
    // textContent : getText rend vide le libellé rogné d'un onglet étroit.
    expect(await $(".tab .label").getProperty("textContent")).toContain("ttyA @ 9600");
    // Le panneau SFTP n'a pas de sens sur un port série.
    expect(await $("#sftp-toggle").isEnabled()).toBe(false);

    // La commande est retapée jusqu'à ce que son écho revienne : le relais du
    // port peut n'être pas tout à fait prêt à l'instant où la session passe
    // « live », et une frappe synthétique perd parfois un caractère (vu une
    // fois en intégration continue, l'écho n'arrivait jamais). Le socat renvoie
    // tout ce qu'il reçoit, retaper est donc sans effet de bord.
    await browser.waitUntil(async () => {
      await browser.execute(() => document.querySelector(".xterm-helper-textarea")?.focus());
      await browser.keys("show version");
      await browser.keys("Enter");
      await browser.pause(400);
      return (await sortiePty()).includes("show version");
    }, { timeout: 15000, interval: 1000, timeoutMsg: "l'écho du port série n'est jamais arrivé" });
  });
});
