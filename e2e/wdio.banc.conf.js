// Banc de mesures : mêmes bac à sable, serveurs locaux et pilote que la suite
// bout en bout, mais d'autres spécifications (e2e/banc/) et des délais
// d'exécution longs (les paliers de 30 sessions, l'endurance).
//
// Ce n'est pas un test : le banc ne fait aucune assertion sur les chiffres. Il
// verse ses échantillons dans BANC_SORTIE, que scripts/banc.mjs compare. Il se
// lance par ce script, jamais par `npm test` : ces fichiers ne sont pas dans la
// liste des spécifications de la suite, et le compteur de scénarios ne les
// voit pas.
//
// Conception : docs/superpowers/specs/2026-11-27-banc-et-audit-design.md.
import { config as base } from "./wdio.conf.js";

export const config = {
  ...base,
  specs: (process.env.BANC_SPECS ?? "./banc/*.spec.js").split(","),
  exclude: [],
  // Un scénario qui échoue (palier de 30 sessions qui ne s'ouvre pas, serveur
  // absent) ne doit pas priver les autres de leur mesure : il le consigne dans
  // `absents` et le banc continue.
  bail: 0,
  mochaOpts: { ...base.mochaOpts, timeout: Number(process.env.BANC_DELAI_MS ?? 3_600_000) },
};
