// Enregistrer un hôte SSH puis se connecter dans la foulée.
//
// Signalé en usage réel : l'onglet s'intitulait « utilisateur@adresse » au lieu
// de l'alias saisi, et la session n'était rattachée à aucune ligne de la barre
// latérale. Il fallait fermer l'onglet et se reconnecter depuis la liste pour
// retrouver le bon nom.
import { findHostRow } from "./helpers.js";
import { SSH_PORT, CLE_CLIENTE } from "../wdio.conf.js";
import { userInfo } from "node:os";

describe("Enregistrer un hôte puis se connecter", () => {
  const ALIAS = "hote-nomme";

  it("l'onglet porte l'alias saisi, pas « utilisateur@adresse »", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await $("#m-addr").setValue("127.0.0.1");
    await $("#m-port").setValue(String(SSH_PORT));
    await $("#m-user").setValue(userInfo().username);

    // Authentification par clé : celle que le harnais a semée pour test-ssh.
    await browser.execute(() => {
      const r = document.querySelector('input[name="auth"][value="key"]');
      r.checked = true; r.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await $("#m-key").setValue(CLE_CLIENTE);

    // Enregistrer sous un alias, et se connecter dans la foulée.
    await browser.execute((a) => {
      document.getElementById("m-save").checked = true;
      document.getElementById("m-save").dispatchEvent(new Event("change", { bubbles: true }));
      document.getElementById("m-alias").value = a;
    }, ALIAS);
    await $("#m-submit").click();

    // L'onglet doit porter l'alias — c'est tout l'objet du scénario.
    await browser.waitUntil(
      async () => (await $$(".tab .label")).length > 0,
      { timeout: 20000, timeoutMsg: "aucun onglet ouvert" },
    );
    const libelle = await browser.execute(() =>
      document.querySelector(".tab.active .label")?.textContent ?? null);
    expect(libelle).toBe(ALIAS);

    // Et l'hôte doit exister dans la barre latérale sous ce nom.
    expect(await findHostRow(ALIAS)).not.toBe(null);
  });
});
