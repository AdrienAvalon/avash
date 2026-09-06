// Clés SSH : la fenêtre « Mes clés SSH » génère une paire ed25519 dans le
// ~/.ssh du bac à sable, la liste la montre, et un second essai sous le même
// nom est refusé plutôt que d'écraser la clé. La mesure de couverture du
// 06/09/2026 a montré que rien, dans la suite, ne traversait ces commandes
// (`keys_list`, `key_generate`) : une fonction phare sans scénario.
//
// Le déploiement (`key_deploy`, l'équivalent de ssh-copy-id) ne se joue pas
// ici : il exige une connexion par mot de passe, que le sshd du harnais, non
// root et à clé seule, ne sait pas offrir. Il est couvert par les tests
// d'intégration du cœur contre leur propre serveur.
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { CLE_CLIENTE } from "../wdio.conf.js";
import { trouverLigne } from "./helpers.js";

const NOM = "e2e-cle";

describe("Clés SSH — génération depuis la fenêtre", () => {
  it("génère une paire ed25519, la liste, et refuse d'en écraser une", async () => {
    await $("#keys-btn").click();
    await $("#keys-modal").waitForDisplayed({ timeout: 5000 });
    // Le formulaire vit dans un bloc replié : sans l'ouvrir, le champ n'est
    // pas interactif pour le pilote.
    await browser.execute(() => document.querySelector("#keys-modal details.key-block")?.setAttribute("open", ""));

    await $("#k-name").setValue(NOM);
    await $("#k-gen-submit").click();
    await browser.waitUntil(async () => (await $("#k-ok").getAttribute("hidden")) === null,
      { timeout: 10000, timeoutMsg: "la création de la clé n'a pas été confirmée" });
    expect(await $("#k-ok").getText()).toContain(NOM);

    // La liste la montre, et la paire est bien sur le disque, dans le ~/.ssh du
    // bac à sable (celui de la clé cliente du sshd).
    const ligne = () => trouverLigne("#key-list .key-row", ".kname", NOM);
    await browser.waitUntil(async () => (await ligne()) !== null, { timeout: 8000, timeoutMsg: "clé absente de la liste" });
    const dossier = dirname(CLE_CLIENTE);
    expect(existsSync(join(dossier, NOM))).toBe(true);
    expect(existsSync(join(dossier, `${NOM}.pub`))).toBe(true);

    // Même nom une seconde fois : refus explicite, la clé existante reste.
    await $("#k-name").setValue(NOM);
    await $("#k-gen-submit").click();
    await browser.waitUntil(async () => (await $("#k-error").getAttribute("hidden")) === null,
      { timeout: 10000, timeoutMsg: "le second essai n'a pas été refusé" });
    expect((await $("#k-error").getText()).length).toBeGreaterThan(0);

    await $("#k-close").click();
    await browser.waitUntil(async () => !(await $("#keys-modal").isDisplayed()), { timeout: 5000 });
  });
});
