// Comparateur du banc : deux résultats (base, tête) → un verdict par métrique
// et un tableau Markdown.
//
// Usage : node e2e/banc/comparer.js <base.json> <tete.json>
//           [--seuils e2e/banc/seuils.json] [--markdown sortie.md] [--informatif]
//
// Code de sortie 1 si une métrique GARDÉE (seuils.json) régresse, 0 sinon ;
// toujours 0 avec --informatif (publie le tableau sans bloquer).
//
// Le verdict demande deux choses pour parler de régression : un écart des
// médianes au-delà du seuil, ET des dispersions ([p25, p75]) disjointes. Un
// seul de ces critères laisserait passer le bruit d'une machine partagée :
// une médiane bouge de 15 % sur un exécuteur de CI sans que rien n'ait changé.
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { stats, lireResultat } from "./commun.js";

/** Une unité de la métrique (1 ms, 1 Mo, 1 i/s) : en dessous, une médiane
 *  n'est plus un dénominateur sûr (0 ms d'écho, arrondi du bandeau). */
const PLANCHER = 1;
const MIN_ECHANTILLONS = 3;

export function comparer(a, b, seuils) {
  const cles = [...new Set([...Object.keys(a.metriques), ...Object.keys(b.metriques)])].sort();
  return cles.map((cle) => {
    const ma = a.metriques[cle];
    const mb = b.metriques[cle];
    const regle = { ...seuils.defaut, ...(seuils.metriques?.[cle] ?? {}) };
    const sa = ma ? stats(ma.echantillons) : null;
    const sb = mb ? stats(mb.echantillons) : null;
    const meta = ma ?? mb;
    const ligne = { cle, unite: meta.unite, sens: meta.sens, a: sa, b: sb, ecart: null, verdict: "absent", garde: !!regle.garde };
    if (!sa || !sb || sa.n < MIN_ECHANTILLONS || sb.n < MIN_ECHANTILLONS) return ligne;
    let ecart = (sb.mediane - sa.mediane) / Math.max(Math.abs(sa.mediane), PLANCHER);
    if (meta.sens === "haut") ecart = -ecart;
    ligne.ecart = ecart;
    const chevauchent = sa.p25 <= sb.p75 && sb.p25 <= sa.p75;
    if (chevauchent || Math.abs(ecart) < regle.seuil) ligne.verdict = "bruit";
    else ligne.verdict = ecart > 0 ? "pire" : "mieux";
    return ligne;
  });
}

/** Les métriques d'un seul résultat qui franchissent un plafond absolu
 *  (`seuils.absolus`, par exemple la pente de la mémoire en endurance : une
 *  fuite se juge seule, sans référence). Rend la liste des dépassements. */
export function depassementsAbsolus(resultat, seuils) {
  const out = [];
  for (const [cle, regle] of Object.entries(seuils.absolus ?? {})) {
    const m = resultat.metriques[cle];
    const s = m ? stats(m.echantillons) : null;
    if (!s) continue;
    if (regle.max !== undefined && s.mediane > regle.max) out.push(`${cle} = ${s.mediane.toFixed(1)} ${m.unite} > ${regle.max}`);
    if (regle.min !== undefined && s.mediane < regle.min) out.push(`${cle} = ${s.mediane.toFixed(1)} ${m.unite} < ${regle.min}`);
  }
  return out;
}

const fmt = (v) => (v === null || v === undefined ? "–" : Number.isInteger(v) ? String(v) : v.toFixed(v < 10 ? 2 : 1));
const charge = (c) => (c ? `${fmt(c.valeur)} (${c.type})` : "non relevée");
const SYMBOLE = { mieux: "✅ mieux", pire: "❌ pire", bruit: "≈ bruit", absent: "– absent" };

export function enMarkdown(lignes, a, b) {
  const out = [];
  out.push(`## Banc : ${a.reference} (${a.commit.slice(0, 9)}) contre ${b.reference} (${b.commit.slice(0, 9)})`);
  out.push("");
  out.push(`Plateforme : ${b.plateforme}, ${b.machine.processeur}, ${b.machine.coeurs} cœurs, ${b.machine.memoireGo} Go.`);
  out.push(`Charge : base ${charge(a.machine.chargeDepart)} → ${charge(a.machine.chargeFin)} ; tête ${charge(b.machine.chargeDepart)} → ${charge(b.machine.chargeFin)}.`);
  out.push("");
  out.push("| Métrique | Unité | Base (médiane, p25–p75) | Tête (médiane, p25–p75) | Écart | Verdict |");
  out.push("|---|---|---|---|---:|---|");
  for (const l of lignes) {
    const cote = (s) => (s ? `${fmt(s.mediane)} (${fmt(s.p25)}–${fmt(s.p75)}, n=${s.n})` : "–");
    const ecart = l.ecart === null ? "–" : `${l.ecart > 0 ? "+" : ""}${(l.ecart * 100).toFixed(1)} %`;
    out.push(`| \`${l.cle}\`${l.garde ? " 🛡" : ""} | ${l.unite} | ${cote(l.a)} | ${cote(l.b)} | ${ecart} | ${SYMBOLE[l.verdict]} |`);
  }
  out.push("");
  out.push("Écart positif = pire, quel que soit le sens de la métrique. 🛡 = métrique gardée (bloque la PR si elle régresse).");
  const absents = [...a.absents.map((x) => `base : ${x}`), ...b.absents.map((x) => `tête : ${x}`)];
  if (absents.length) {
    out.push("");
    out.push("Scénarios non mesurés :");
    for (const x of absents) out.push(`- ${x}`);
  }
  return out.join("\n") + "\n";
}

function principal(argv) {
  const pos = argv.filter((x, i) => !x.startsWith("--") && !argv[i - 1]?.startsWith("--seuils") && !argv[i - 1]?.startsWith("--markdown"));
  const opt = (nom) => { const i = argv.indexOf(nom); return i >= 0 ? argv[i + 1] : undefined; };
  if (pos.length !== 2) {
    console.error("usage : comparer.js <base.json> <tete.json> [--seuils f] [--markdown f] [--informatif]");
    return 2;
  }
  const seuils = JSON.parse(readFileSync(opt("--seuils") ?? new URL("./seuils.json", import.meta.url), "utf8"));
  const a = lireResultat(pos[0]);
  const b = lireResultat(pos[1]);
  const lignes = comparer(a, b, seuils);
  const md = enMarkdown(lignes, a, b);
  process.stdout.write(md);
  if (opt("--markdown")) writeFileSync(opt("--markdown"), md);
  if (argv.includes("--informatif")) return 0;
  return lignes.some((l) => l.verdict === "pire" && l.garde) ? 1 : 0;
}

if (process.argv[1] === fileURLToPath(import.meta.url)) process.exitCode = principal(process.argv.slice(2));
