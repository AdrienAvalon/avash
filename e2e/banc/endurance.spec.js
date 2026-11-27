// Banc S8 : endurance. Dix sessions vivantes pendant BANC_DUREE_MIN minutes
// (30 par défaut), dont une qui écrit sans arrêt : la mémoire de
// l'application ne doit pas monter de façon régulière (fuite), et la frappe
// doit rester aussi vive à la fin qu'au début.
//
// Ne tourne que la nuit (scripts/banc.mjs --jeu endurance) : trop long pour
// une PR.
import { verser } from "./commun.js";
import { pente } from "./commun.js";
import { poserSonde, ouvrirSessionSsh, serieDeFrappes, releveApplication, taperTexte, activerOnglet } from "./sondes.js";

const DUREE_MIN = Number(process.env.BANC_DUREE_MIN ?? 30);
const SESSIONS = 10;
const PAS_S = 30;

describe("Banc : endurance", () => {
  it(`S8 : ${SESSIONS} sessions pendant ${DUREE_MIN} min`, async () => {
    await poserSonde();
    for (let i = 0; i < SESSIONS; i++) await ouvrirSessionSsh();
    // La première session écrit un point par seconde : rien d'autre que des
    // points, pour que l'écho des frappes mesurées ailleurs ne s'y confonde
    // jamais (la sonde reconnaît l'écho à la lettre tapée).
    await activerOnglet(0);
    await taperTexte("while true; do printf .; sleep 1; done");
    await browser.keys(["Enter"]);
    await activerOnglet(SESSIONS - 1);

    const debut = await serieDeFrappes(20);
    verser("S8/echo-debut", { unite: "ms", sens: "bas" }, debut.map((f) => f.echo));

    const points = [];
    const t0 = Date.now();
    while (Date.now() - t0 < DUREE_MIN * 60_000) {
      const r = await releveApplication();
      if (r.memoireMo !== null) points.push({ t: (Date.now() - t0) / 1000, v: r.memoireMo });
      await browser.pause(PAS_S * 1000);
    }
    const p = pente(points);
    verser("S8/pente-memoire", { unite: "Mo/h", sens: "bas" }, [p]);
    verser("S8/memoire-fin", { unite: "Mo", sens: "bas" }, [points.at(-1)?.v ?? null]);

    const fin = await serieDeFrappes(20);
    verser("S8/echo-fin", { unite: "ms", sens: "bas" }, fin.map((f) => f.echo));
  });
});
