// Enregistrer un hôte SSH puis se connecter dans la foulée.
//
// Signalé en usage réel : l'onglet s'intitulait « utilisateur@adresse » au lieu
// de l'alias saisi, et la session n'était rattachée à aucune ligne de la barre
// latérale. Il fallait fermer l'onglet et se reconnecter depuis la liste pour
// retrouver le bon nom.
import { findHostRow, startRdpServer, startVncServer, waitForPort, attendreBureauConnecte } from "./helpers.js";
import { SSH_PORT, CLE_CLIENTE } from "../wdio.conf.js";
import { userInfo } from "node:os";

// La pastille verte vit sur la ligne de l'hôte, que renderHosts() reconstruit
// entièrement quand une session devient vivante. Un handle capturé avant ce
// rendu pointe alors un noeud détaché — inerte sous le pilote WebDriver
// embarqué (Windows, macOS) : on re-cherche la ligne à chaque tour plutôt que
// d'interroger un handle figé (même piège que l'aléa « vue-partagee »).
async function attendrePastilleVerte(chercher, quoi) {
  await browser.waitUntil(async () => {
    const ligne = await chercher();
    if (!ligne) return false;
    return (await ligne.$(".dot.live")).isExisting();
  }, { timeout: 20000, timeoutMsg: `pastille verte absente sur ${quoi} tout juste enregistré` });
}

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

    // Et l'hôte doit exister dans la barre latérale sous ce nom. L'exécuteur
    // Windows a été vu mettre plus de huit secondes à l'afficher (chaîne du
    // 05/09/2026, un passage sur deux) : on attend plus longtemps, et l'échec
    // nomme ce que la barre montrait.
    try {
      await findHostRow(ALIAS, 20000);
    } catch (e) {
      const presents = await browser.execute(() =>
        [...document.querySelectorAll("#host-list .host .alias")].map((a) => a.textContent));
      throw new Error(`hôte « ${ALIAS} » absent de la barre ; présents : ${JSON.stringify(presents)}`, { cause: e });
    }
    // Et sa pastille doit être verte : la session ouverte est rattachée à la
    // ligne, sans fermer l'onglet et se reconnecter depuis la liste.
    await attendrePastilleVerte(() => findHostRow(ALIAS, 5000).catch(() => null), "l'hôte SSH");
  });
});

// Même exigence pour un bureau RDP enregistré depuis la connexion directe.
// Signalé le 11 septembre 2026 en usage réel : l'onglet s'intitulait
// « utilisateur@adresse » et la ligne de la barre restait sans pastille ; il
// fallait fermer l'onglet et rouvrir depuis la liste pour que tout soit normal.
describe("Enregistrer un bureau RDP puis se connecter", () => {
  const RDP_PORT = 33902;
  const NOM = "bureau-nomme";
  let srv;
  before(async () => { srv = startRdpServer(RDP_PORT); await waitForPort(RDP_PORT); });
  after(() => { if (srv) srv.kill(); });

  it("l'onglet porte le nom saisi et la ligne de la barre a sa pastille verte", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => {
      const r = document.querySelector('input[name="proto"][value="rdp"]');
      r.checked = true;
      r.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await $("#m-addr").setValue("127.0.0.1");
    await $("#m-port").setValue(String(RDP_PORT));
    await $("#m-user").setValue("test");
    await $("#m-password").setValue("test");
    await browser.execute((nom) => {
      const c = document.getElementById("m-rdp-save");
      c.checked = true;
      c.dispatchEvent(new Event("change", { bubbles: true }));
      document.getElementById("m-rdp-name").value = nom;
    }, NOM);
    await $("#m-submit").click();

    await attendreBureauConnecte();
    const libelle = await browser.execute(() =>
      document.querySelector(".tab.active .label")?.textContent ?? null);
    expect(libelle).toBe(NOM);

    await findHostRow(NOM, 20000);
    await attendrePastilleVerte(() => findHostRow(NOM, 5000).catch(() => null), "le bureau RDP");
  });
});

// Et pour un bureau VNC : même volet de la connexion directe, même exigence.
// Le port série n'a pas d'enregistrement depuis ce volet ; l'hôte SSH est
// couvert plus haut, et son alias part à l'ouverture quel que soit le mode
// d'authentification.
describe("Enregistrer un bureau VNC puis se connecter", () => {
  const VNC_PORT = 35905;
  const NOM = "vnc-nomme";
  let srv;
  before(async () => { srv = startVncServer(VNC_PORT); await waitForPort(VNC_PORT); });
  after(() => { if (srv) srv.kill(); });

  it("l'onglet porte le nom saisi et la ligne de la barre a sa pastille verte", async () => {
    await $("#manual-btn").click();
    await $("#manual-modal").waitForDisplayed({ timeout: 5000 });
    await browser.execute(() => {
      const r = document.querySelector('input[name="proto"][value="vnc"]');
      r.checked = true;
      r.dispatchEvent(new Event("change", { bubbles: true }));
    });
    await $("#m-addr").setValue("127.0.0.1");
    await $("#m-port").setValue(String(VNC_PORT));
    await $("#m-password").setValue("test");
    await browser.execute((nom) => {
      const c = document.getElementById("m-rdp-save");
      c.checked = true;
      c.dispatchEvent(new Event("change", { bubbles: true }));
      document.getElementById("m-rdp-name").value = nom;
    }, NOM);
    await $("#m-submit").click();

    await attendreBureauConnecte("le bureau VNC");
    const libelle = await browser.execute(() =>
      document.querySelector(".tab.active .label")?.textContent ?? null);
    expect(libelle).toBe(NOM);

    await findHostRow(NOM, 20000);
    await attendrePastilleVerte(() => findHostRow(NOM, 5000).catch(() => null), "le bureau VNC");
  });
});
