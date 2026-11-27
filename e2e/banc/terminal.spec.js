// Banc S1, S2, S3 et S7 : démarrage, frappe, débit du terminal, connexion SSH.
//
// Reprend et remplace e2e/mesures/latence.spec.js (4 septembre 2026), avec le
// t0 pris dans la page sur toutes les plateformes (voir sondes.js).
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { verser } from "./commun.js";
import {
  poserSonde, mesurerDemarrage, ouvrirSessionSsh, serieDeFrappes, attendreSilence,
  surveillerImages, arreterSurveillance, taperTexte,
} from "./sondes.js";
import { ecouterSortiePty, sortiePty } from "../specs/helpers.js";

const NB_FRAPPES = 40;
const NB_FLOTS = 3;
const TAILLE_FLOT = 50 * 1024 * 1024;

/** Un fichier texte de `taille` octets dans le bac à sable, que le shell
 *  distant (le sshd local, même utilisateur) peut lire. */
function fichierDeFlot(taille) {
  const chemin = join(process.env.AVASH_E2E_SANDBOX, "flot.txt");
  const ligne = (i) => `ligne ${String(i).padStart(7, "0")} ${"abcdefghijklmnopqrstuvwxyz0123456789".repeat(2)}\n`;
  const morceaux = [];
  let total = 0;
  for (let i = 0; total < taille; i++) { const l = ligne(i); morceaux.push(l); total += l.length; }
  writeFileSync(chemin, morceaux.join(""));
  return { chemin: chemin.replace(/\\/g, "/"), octets: total };
}

describe("Banc : terminal", () => {
  before(async () => {
    await ecouterSortiePty();
    await poserSonde();
    await mesurerDemarrage(verser);
  });

  it("S2 et S7 : connexion SSH, puis frappe dans une session seule", async () => {
    verser("S7/connexion-ssh", { unite: "ms", sens: "bas" }, [await ouvrirSessionSsh()]);
    const frappes = await serieDeFrappes(NB_FRAPPES);
    verser("S2/echo-1-session", { unite: "ms", sens: "bas" }, frappes.map((f) => f.echo));
    verser("S2/image-1-session", { unite: "ms", sens: "bas" }, frappes.map((f) => f.image));
  });

  it("S3 : débit du terminal (cat de 50 Mo)", async () => {
    const flot = fichierDeFlot(TAILLE_FLOT);
    const durees = [];
    const pires = [];
    for (let i = 0; i < NB_FLOTS; i++) {
      // Une session neuve par flot : la ligne de commande de la précédente
      // porte encore les lettres tapées, et le défilement de 50 Mo.
      verser("S7/connexion-ssh", { unite: "ms", sens: "bas" }, [await ouvrirSessionSsh()]);
      await taperTexte(`cat '${flot.chemin}'`);
      await attendreSilence(500);
      await surveillerImages();
      await browser.execute(() => { const m = window.__banc; m.marque = performance.now(); m.caracteres = 0; });
      await browser.keys(["Enter"]);
      try {
        await browser.waitUntil(
          () => browser.execute((n) => window.__banc.caracteres >= n, flot.octets),
          { timeout: 180000, interval: 200 },
        );
      } catch (e) {
        const recu = await browser.execute(() => window.__banc.caracteres);
        throw new Error(`flot incomplet : ${recu} caractères reçus sur ${flot.octets} ; fin du terminal :\n${(await sortiePty()).slice(-400)}`, { cause: e });
      }
      await attendreSilence(1000, 600000);
      durees.push(await browser.execute(() => window.__banc.derniereSortie - window.__banc.marque));
      pires.push((await arreterSurveillance()).pireImageMs);
    }
    verser("S3/flot-50mo", { unite: "ms", sens: "bas" }, durees);
    verser("S3/pire-image-flot", { unite: "ms", sens: "bas" }, pires);
  });
});
