import { createConnection } from "node:net";
import { LOCAL_SERVERS, SSH_PORT } from "../wdio.conf.js";
import { trouverLigne } from "./helpers.js";

/** La bannière que sert un port TCP (ce qu'un serveur SSH envoie en premier). */
function banniere(port) {
  return new Promise((resolve, reject) => {
    const s = createConnection({ host: "127.0.0.1", port }, () => {});
    let recu = "";
    s.setTimeout(8000, () => { s.destroy(); reject(new Error(`rien lu sur le port ${port}`)); });
    s.on("data", (d) => { recu += d.toString(); if (recu.includes("\n")) { s.destroy(); resolve(recu.trim()); } });
    s.on("error", reject);
  });
}

// Le tunnel est réellement ouvert : une redirection locale vers le sshd du
// harnais, à travers la session SSH de « test-ssh », et le port local sert la
// bannière du sshd. La mesure de couverture du 06/09/2026 a montré que le
// scénario ci-dessous ne faisait que créer une définition : `tunnel_start`
// n'était jamais traversé, alors que c'est lui qui ouvre la session, écoute le
// port et relaie.
describe("Tunnels — démarrer un tunnel local vers le sshd du harnais", () => {
  const PORT_LOCAL = 18081;
  const findRow = () => trouverLigne("#tunnel-list .tunnel-row", ".tname", "Tunnel sshd");

  before(function () { if (!LOCAL_SERVERS) this.skip(); });

  after(async () => {
    // Quoi qu'il arrive, ne pas laisser un tunnel ouvert, ni une définition,
    // ni la fenêtre ouverte (elle intercepterait les clics du scénario suivant).
    if (!(await $("#tunnels-modal").isDisplayed())) await $("#tunnels-btn").click();
    await $("#tunnels-modal").waitForDisplayed({ timeout: 5000 });
    const row = await findRow().catch(() => null);
    if (row) {
      if ((await row.getAttribute("class")).includes("alive")) {
        await row.$('[data-act="toggle"]').click();
        await browser.pause(1000);
      }
      await (await findRow()).$('[data-act="delete"]').click();
      await $("#confirm-modal").waitForDisplayed({ timeout: 5000 });
      await $("#confirm-ok").click();
      await browser.waitUntil(async () => (await findRow()) === null, { timeout: 8000 });
    }
    await $("#t-close").click();
    await browser.waitUntil(async () => !(await $("#tunnels-modal").isDisplayed()), { timeout: 5000 });
  });

  it("ouvre la session, écoute le port local, relaie jusqu'au sshd, puis s'arrête", async () => {
    await $("#tunnels-btn").click();
    await $("#tunnels-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => document.getElementById("tunnel-block").setAttribute("open", ""));
    // Le choix de l'hôte se pose par le DOM : sous le serveur WebDriver
    // embarqué (Windows), le clic sur une option n'atteint pas le formulaire,
    // et le tunnel partait vers le premier hôte de la liste, injoignable.
    await browser.execute(() => {
      const s = document.getElementById("t-alias");
      s.value = "test-ssh";
      s.dispatchEvent(new Event("change", { bubbles: true }));
    });
    expect(await $("#t-alias").getValue()).toBe("test-ssh");
    await $("#t-bind").setValue(String(PORT_LOCAL));
    await $("#t-host").setValue("127.0.0.1");
    await $("#t-port").setValue(String(SSH_PORT));
    await $("#t-name").setValue("Tunnel sshd");
    await $("#t-submit").click();
    await browser.waitUntil(async () => (await findRow()) !== null, { timeout: 8000, timeoutMsg: "tunnel non listé" });

    // Démarrer : « test-ssh » s'authentifie par clé, aucun mot de passe demandé.
    await (await findRow()).$('[data-act="toggle"]').click();
    await browser.waitUntil(async () => ((await (await findRow())?.getAttribute("class")) ?? "").includes("alive"),
      { timeout: 20000, timeoutMsg: "le tunnel n'est pas passé « vivant »" }).catch(async (e) => {
        // L'erreur que la ligne affiche vaut mieux qu'un délai muet.
        const erreur = await (await findRow())?.$(".terr").getProperty("textContent").catch(() => "");
        throw new Error(`${e.message} ${erreur ? `(la ligne dit : ${erreur})` : ""}`);
      });

    // Le port local est bien servi par le sshd, à travers la session.
    expect(await banniere(PORT_LOCAL)).toMatch(/^SSH-2\.0-/);
    // Le trafic se voit dans la ligne (une connexion relayée au moins) ; la
    // fenêtre rafraîchit l'état toutes les 1,5 s. textContent : getText rend
    // vide un libellé rogné.
    await browser.waitUntil(async () => {
      const stats = await (await findRow()).$(".tstats").getProperty("textContent");
      // « 1 au total · ↑0 o ↓41 o » : une connexion comptée, la bannière reçue.
      return /[1-9]\d* (au total|conn)/.test(stats) && /↓[1-9]/.test(stats);
    }, { timeout: 10000, timeoutMsg: "le trafic de la connexion relayée n'apparaît pas" });

    // Arrêter : le port ne répond plus.
    await (await findRow()).$('[data-act="toggle"]').click();
    await browser.waitUntil(async () => !((await (await findRow())?.getAttribute("class")) ?? "alive").includes("alive"),
      { timeout: 10000, timeoutMsg: "le tunnel ne s'est pas arrêté" });
    await expect(banniere(PORT_LOCAL)).rejects.toThrow();
  });
});

describe("Tunnels — créer puis supprimer une définition", () => {
  it("crée un tunnel local, le voit dans la liste, le supprime (askConfirm)", async () => {
    await $("#tunnels-btn").click();
    await $("#tunnels-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => document.getElementById("tunnel-block").setAttribute("open", ""));
    await $("#t-alias").selectByIndex(0);       // un hôte semé (web-1/db-1)
    await $("#t-bind").setValue("18080");
    await $("#t-host").setValue("localhost");
    await $("#t-port").setValue("5432");
    await $("#t-name").setValue("Tunnel E2E");
    await $("#t-submit").click();

    // `trouverLigne` tolère une reconstruction de la liste pendant le parcours :
    // une référence caduque levait une erreur qui avortait le `waitUntil`.
    const findRow = () => trouverLigne("#tunnel-list .tunnel-row", ".tname", "Tunnel E2E");
    await browser.waitUntil(async () => (await findRow()) !== null, { timeout: 8000, timeoutMsg: "tunnel non listé" });

    await (await findRow()).$('[data-act="delete"]').click();
    await $("#confirm-modal").waitForDisplayed({ timeout: 5000 });
    await $("#confirm-ok").click();
    await browser.waitUntil(async () => (await findRow()) === null, { timeout: 8000, timeoutMsg: "tunnel pas supprimé" });
  });
});
