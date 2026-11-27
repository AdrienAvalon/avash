// Banc S5 et S7 : le bureau à distance seul au premier plan.
//
// Reprend e2e/mesures/rdp.spec.js (26 novembre 2026, décodeur RemoteFX : 10 →
// 25 i/s) : la mire du serveur de test pousse un plein écran 1920×1080 qui
// change entièrement à chaque image, à la cadence MIRE_HZ (30 par défaut).
// Le processeur se relève sur tout l'arbre de l'application (processus RDP et
// page compris), sous Linux comme sous Windows.
//
// Sur un xrdp (BANC_XRDP) ou un Windows (BANC_RDP_WINDOWS) réels, l'écran de
// connexion ou le bureau est statique : on n'y mesure que le temps jusqu'à la
// première image (S7) et le processeur au repos.
import { verser } from "./commun.js";
import { poserSonde, mesurerDemarrage, connecterRdp, lireBandeauRdp, processeurSur, activerOnglet } from "./sondes.js";
import { startRdpServer, waitForPort } from "../specs/helpers.js";

const PORT = 33911;
const HZ = Number(process.env.MIRE_HZ ?? 30);
const NB_CONNEXIONS = 3;

function serveur(variable) {
  const v = process.env[variable];
  if (!v) return null;
  const [hote, port, utilisateur] = v.split(":");
  return { hote, port: Number(port), utilisateur, mdp: process.env[`${variable}_MDP`] ?? "" };
}

/** Ferme l'onglet actif, en confirmant si la question vient. */
async function fermerActif() {
  const avant = (await $$(".tab")).length;
  await browser.execute(() => document.querySelector(".tab.active .close")?.dispatchEvent(new MouseEvent("click", { bubbles: true })));
  await browser.pause(300);
  if (await browser.execute(() => !!document.querySelector("#confirm-modal.open"))) await $("#confirm-ok").click();
  await browser.waitUntil(async () => (await $$(".tab")).length < avant, { timeout: 10000, timeoutMsg: "onglet jamais fermé" });
}

describe("Banc : bureau à distance", () => {
  let srv;
  before(async () => {
    srv = startRdpServer(PORT, null, ["--mire", String(HZ)]);
    await waitForPort(PORT);
    await poserSonde();
    await mesurerDemarrage(verser);
  });
  after(() => { if (srv) srv.kill(); });

  it(`S5 : mire plein écran à ${HZ} i/s`, async () => {
    for (let i = 0; i < NB_CONNEXIONS; i++) {
      verser("S7/premiere-image-mire", { unite: "ms", sens: "bas" },
        [await connecterRdp({ hote: "127.0.0.1", port: PORT, utilisateur: "test", mdp: "test" })]);
      if (i < NB_CONNEXIONS - 1) await fermerActif();
    }
    await activerOnglet(0);
    await browser.pause(3000);
    const releves = [];
    const processeur = processeurSur(10000);
    const t0 = Date.now();
    while (Date.now() - t0 < 10000) {
      const b = await lireBandeauRdp();
      if (b) releves.push(b);
      await browser.pause(500);
    }
    verser("S5/fps-mire", { unite: "i/s", sens: "haut" }, releves.map((r) => r.fps));
    verser("S5/debit-mire", { unite: "Mo/s", sens: "haut" }, releves.map((r) => r.moParS));
    verser("S5/aller-retour-mire", { unite: "ms", sens: "bas" }, releves.map((r) => r.ms));
    verser("S5/processeur-mire", { unite: "% d'un cœur", sens: "bas" }, [await processeur]);
    await fermerActif();
  });

  for (const [variable, cle] of [["BANC_XRDP", "xrdp"], ["BANC_RDP_WINDOWS", "windows"]]) {
    it(`S5 et S7 : ${cle} réel`, async function () {
      const s = serveur(variable);
      if (!s) this.skip();
      for (let i = 0; i < NB_CONNEXIONS; i++) {
        verser(`S7/premiere-image-${cle}`, { unite: "ms", sens: "bas" }, [await connecterRdp(s)]);
        await browser.pause(2000);
        if (i < NB_CONNEXIONS - 1) await fermerActif();
      }
      const repos = [];
      for (let i = 0; i < 3; i++) repos.push(await processeurSur(2000));
      verser(`S5/processeur-repos-${cle}`, { unite: "% d'un cœur", sens: "bas" }, repos);
      await fermerActif();
    });
  }
});
