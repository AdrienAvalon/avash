// Banc S4 : l'usage réel du mainteneur, environ 10 sessions ouvertes en même
// temps (SSH, RDP vers Windows, xrdp, SFTP), puis 20 et 30 pour voir où ça
// cède avant qu'il y arrive (27 novembre 2026).
//
// Paliers cumulatifs (BANC_PALIERS, « 10,20,30 » par défaut). Composition du
// premier palier : une mire RDP (test-rdp-server, 1080p à 30 Hz), un xrdp
// (BANC_XRDP=hôte:port:utilisateur, mot de passe dans BANC_XRDP_MDP), un
// Windows (BANC_RDP_WINDOWS, BANC_RDP_WINDOWS_MDP), le reste en SSH ; un
// serveur absent est remplacé par une session SSH de plus. Un transfert SFTP
// de 200 Mo tourne pendant la mesure de la frappe.
import { randomBytes } from "node:crypto";
import { closeSync, mkdirSync, openSync, rmSync, writeSync } from "node:fs";
import { join } from "node:path";
import { verser, absent } from "./commun.js";
import {
  poserSonde, mesurerDemarrage, ouvrirSessionSsh, serieDeFrappes, releveApplication,
  processeurSur, connecterRdp, lireBandeauRdp, activerOnglet, cheminDistant,
  lancerTelechargementSftp, attendreTransfert,
} from "./sondes.js";
import { startRdpServer, waitForPort } from "../specs/helpers.js";

const PORT_MIRE = 33912;
const PALIERS = (process.env.BANC_PALIERS ?? "10,20,30").split(",").map(Number);
const NOM_SFTP = "banc-sftp-200mo.bin";

/** « hôte:port:utilisateur » et son mot de passe, ou null. */
function serveur(variable) {
  const v = process.env[variable];
  if (!v) return null;
  const [hote, port, utilisateur] = v.split(":");
  return { hote, port: Number(port), utilisateur, mdp: process.env[`${variable}_MDP`] ?? "" };
}

/** 200 Mo aléatoires (incompressibles), écrits par blocs dans un sous-dossier
 *  du bac à sable : le dossier de réception peut être le bac à sable lui-même
 *  (repli du cœur sans dossier de téléchargements), la source ne doit pas s'y
 *  trouver. */
function fichierSftp() {
  const dossier = join(process.env.AVASH_E2E_SANDBOX, "banc-source");
  mkdirSync(dossier, { recursive: true });
  const fd = openSync(join(dossier, NOM_SFTP), "w");
  const bloc = randomBytes(4 * 1024 * 1024);
  for (let i = 0; i < 50; i++) writeSync(fd, bloc);
  closeSync(fd);
  return dossier;
}

/** Le fichier reçu, à retirer après chaque transfert : le bac à sable vit
 *  souvent sur un tmpfs (mémoire vive). */
function retirerRecu() {
  const sandbox = process.env.AVASH_E2E_SANDBOX;
  for (const d of ["Downloads", "Téléchargements", ""]) rmSync(join(sandbox, d, NOM_SFTP), { force: true });
}

describe("Banc : sessions multiples", () => {
  let mire;
  let dossierSftp;
  let ouvertes = 0;
  let rangMire = null;

  before(async () => {
    mire = startRdpServer(PORT_MIRE, null, ["--mire", "30"]);
    await waitForPort(PORT_MIRE);
    dossierSftp = fichierSftp();
    await poserSonde();
    await mesurerDemarrage(verser);
  });

  after(() => {
    if (mire) mire.kill();
    rmSync(join(process.env.AVASH_E2E_SANDBOX, "banc-source"), { recursive: true, force: true });
    retirerRecu();
  });

  for (const n of PALIERS) {
    it(`palier de ${n} sessions`, async () => {
      const ouvertures = [];
      try {
        if (ouvertes === 0) {
          rangMire = 0;
          verser("S7/premiere-image-mire", { unite: "ms", sens: "bas" },
            [await connecterRdp({ hote: "127.0.0.1", port: PORT_MIRE, utilisateur: "test", mdp: "test" })]);
          ouvertes++;
          for (const [variable, cle] of [["BANC_XRDP", "xrdp"], ["BANC_RDP_WINDOWS", "windows"]]) {
            const s = serveur(variable);
            if (!s) continue;
            verser(`S7/premiere-image-${cle}`, { unite: "ms", sens: "bas" }, [await connecterRdp(s)]);
            ouvertes++;
          }
        }
        while (ouvertes < n) {
          const d = await ouvrirSessionSsh();
          ouvertures.push(d);
          verser("S7/connexion-ssh", { unite: "ms", sens: "bas" }, [d]);
          ouvertes++;
        }
      } catch (e) {
        absent(`S4/palier-${n}`, `${ouvertes} sessions ouvertes sur ${n} : ${e.message.split("\n")[0]}`);
        throw e;
      }
      verser(`S4/ouverture-onglet-${n}`, { unite: "ms", sens: "bas" }, ouvertures);

      // Au repos, onglet SSH actif (le dernier ouvert) : mémoire, processeur.
      await browser.pause(2000);
      const memoire = [];
      for (let i = 0; i < 5; i++) { memoire.push((await releveApplication()).memoireMo); await browser.pause(2000); }
      verser(`S4/memoire-${n}`, { unite: "Mo", sens: "bas" }, memoire);
      const repos = [];
      for (let i = 0; i < 5; i++) repos.push(await processeurSur(2000));
      verser(`S4/processeur-repos-${n}`, { unite: "% d'un cœur", sens: "bas" }, repos);

      // La frappe, avec un transfert SFTP en cours sur la même session.
      await lancerTelechargementSftp(cheminDistant(dossierSftp), NOM_SFTP);
      const frappes = await serieDeFrappes(20);
      verser(`S4/echo-${n}`, { unite: "ms", sens: "bas" }, frappes.map((f) => f.echo));
      const etat = await attendreTransfert(NOM_SFTP);
      if (!etat.includes("fini")) absent(`S4/sftp-${n}`, `transfert terminé en « ${etat} »`);
      retirerRecu();

      // La mire au premier plan : ce que tient le bureau à distance quand
      // dix autres sessions vivent derrière.
      await activerOnglet(rangMire);
      await browser.pause(3000);
      const fps = [];
      for (let i = 0; i < 10; i++) { const b = await lireBandeauRdp(); if (b) fps.push(b.fps); await browser.pause(500); }
      verser(`S4/fps-mire-${n}`, { unite: "i/s", sens: "haut" }, fps);
      await activerOnglet(ouvertes - 1);
    });
  }
});
