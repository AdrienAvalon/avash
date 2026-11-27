// Bibliothèque commune du banc de mesures : statistiques, relevé des processus
// de l'application, format du résultat.
//
// Le banc verse des ÉCHANTILLONS BRUTS, pas des moyennes : c'est le
// comparateur (comparer.js) qui calcule, et il a besoin de la dispersion pour
// séparer une vraie régression du bruit de la machine.
//
// Conception : docs/superpowers/specs/2026-11-27-banc-et-audit-design.md.
import { execFile, execFileSync } from "node:child_process";
import { readFileSync, readdirSync, writeFileSync, existsSync } from "node:fs";
import { promisify } from "node:util";
import { cpus, totalmem, loadavg } from "node:os";

const executer = promisify(execFile);
const WINDOWS = process.platform === "win32";

/** Quantile tel que les mesures historiques le calculaient (`t[floor(p·n)]`,
 *  borné) : les chiffres de la feuille de route restent comparables. */
function quantile(trie, p) {
  return trie[Math.min(trie.length - 1, Math.floor(p * trie.length))];
}

/** Statistiques d'une série ; `null` si elle est vide (les `null` comptent
 *  pour des échantillons manquants, pas pour des zéros). */
export function stats(valeurs) {
  const t = valeurs.filter((v) => v !== null && v !== undefined && Number.isFinite(v)).sort((a, b) => a - b);
  if (!t.length) return null;
  return {
    n: t.length,
    mediane: quantile(t, 0.5),
    p25: quantile(t, 0.25),
    p75: quantile(t, 0.75),
    p95: quantile(t, 0.95),
    min: t[0],
    max: t.at(-1),
  };
}

/** Pente par moindres carrés de `v` en fonction de `t` (secondes), rendue en
 *  unités de `v` par heure ; `null` sous deux points. Sert à l'endurance :
 *  une mémoire qui monte de façon régulière est une fuite. */
export function pente(points) {
  if (points.length < 2) return null;
  const n = points.length;
  const mt = points.reduce((s, p) => s + p.t, 0) / n;
  const mv = points.reduce((s, p) => s + p.v, 0) / n;
  let num = 0;
  let den = 0;
  for (const p of points) {
    num += (p.t - mt) * (p.v - mv);
    den += (p.t - mt) ** 2;
  }
  if (den === 0) return null;
  return (num / den) * 3600;
}

async function powershell(script) {
  const { stdout } = await executer("powershell", ["-NoProfile", "-NonInteractive", "-Command", script], { windowsHide: true });
  return stdout.trim();
}

/** La racine et tous ses descendants. Sous Windows, la page tourne dans des
 *  processus msedgewebview2.exe à part, que seul l'arbre rattache à
 *  l'application : filtrer par nom compterait aussi un Edge ouvert. */
export async function arbreDeProcessus(racinePid) {
  let liens; // [pid, ppid]
  if (WINDOWS) {
    const json = await powershell("Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId | ConvertTo-Json -Compress");
    liens = JSON.parse(json).map((p) => [p.ProcessId, p.ParentProcessId]);
  } else {
    liens = [];
    for (const nom of readdirSync("/proc")) {
      if (!/^\d+$/.test(nom)) continue;
      try {
        // Le nom du processus (entre parenthèses) peut contenir des espaces :
        // on découpe après la dernière parenthèse fermante.
        const stat = readFileSync(`/proc/${nom}/stat`, "utf8");
        const champs = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
        liens.push([Number(nom), Number(champs[1])]);
      } catch { /* parti entre-temps */ }
    }
  }
  const arbre = new Set([racinePid]);
  let ajoute = true;
  while (ajoute) {
    ajoute = false;
    for (const [pid, ppid] of liens) {
      if (!arbre.has(pid) && arbre.has(ppid)) { arbre.add(pid); ajoute = true; }
    }
  }
  return [...arbre];
}

let tic = null;
function ticsParSeconde() {
  if (tic === null) {
    try { tic = Number(execFileSync("getconf", ["CLK_TCK"]).toString().trim()) || 100; } catch { tic = 100; }
  }
  return tic;
}

/** Mémoire (Mo) et temps processeur cumulé (s) d'un ensemble de processus.
 *  Linux : PSS, qui partage équitablement les pages communes entre processus
 *  (la somme des RSS compterait plusieurs fois les bibliothèques de WebKit).
 *  Windows : working set privé (`PrivateMemorySize64`), même raison. Un
 *  processus disparu est ignoré ; aucun vivant donne `null`, jamais zéro. */
export async function releveProcessus(pids) {
  let memoire = 0;
  let processeur = 0;
  let vivants = 0;
  if (WINDOWS) {
    if (!pids.length) return { memoireMo: null, processeurS: null, vivants: 0 };
    const json = await powershell(
      `@(Get-Process -Id ${pids.join(",")} -ErrorAction SilentlyContinue | ForEach-Object { [pscustomobject]@{ m = $_.PrivateMemorySize64; c = $_.TotalProcessorTime.TotalSeconds } }) | ConvertTo-Json -Compress`,
    );
    const liste = json ? [].concat(JSON.parse(json)) : [];
    for (const p of liste) { memoire += p.m; processeur += p.c; vivants++; }
  } else {
    for (const pid of pids) {
      try {
        const rollup = readFileSync(`/proc/${pid}/smaps_rollup`, "utf8");
        const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
        const champs = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
        const pss = Number(/^Pss:\s+(\d+) kB/m.exec(rollup)?.[1] ?? 0);
        memoire += pss * 1024;
        processeur += (Number(champs[11]) + Number(champs[12])) / ticsParSeconde();
        vivants++;
      } catch { /* parti entre-temps, ou smaps_rollup illisible */ }
    }
  }
  if (!vivants) return { memoireMo: null, processeurS: null, vivants: 0 };
  return { memoireMo: memoire / (1024 * 1024), processeurS: processeur, vivants };
}

/** Charge de la machine : moyenne d'une minute sous Linux, pourcentage de
 *  processeur sur 5 s sous Windows (qui n'a pas de loadavg). */
export async function chargeMachine() {
  if (!WINDOWS) return { type: "loadavg-1min", valeur: loadavg()[0] };
  const v = await powershell("(Get-Counter '\\Processor(_Total)\\% Processor Time' -SampleInterval 1 -MaxSamples 5).CounterSamples.CookedValue | Measure-Object -Average | ForEach-Object Average");
  return { type: "processeur-pourcent-5s", valeur: Number(v.replace(",", ".")) };
}

export function nouveauResultat({ reference, commit, plateforme }) {
  return {
    schema: 1,
    reference,
    commit,
    plateforme,
    machine: {
      processeur: cpus()[0]?.model ?? "inconnu",
      coeurs: cpus().length,
      memoireGo: Math.round(totalmem() / 1024 ** 3),
      chargeDepart: null,
      chargeFin: null,
    },
    date: new Date().toISOString(),
    metriques: {},
    absents: [],
  };
}

/** Ajoute des échantillons à une métrique (concatène si elle existe : les
 *  répétitions d'une même référence s'accumulent dans un seul résultat). */
export function ajouter(resultat, cle, { unite, sens }, echantillons) {
  const m = (resultat.metriques[cle] ??= { unite, sens, echantillons: [] });
  m.echantillons.push(...echantillons);
  return resultat;
}

export function ecrireResultat(resultat, chemin) {
  writeFileSync(chemin, JSON.stringify(resultat, null, 2) + "\n");
}

export function lireResultat(chemin) {
  return JSON.parse(readFileSync(chemin, "utf8"));
}

/** Pour les spécifications : verse des échantillons dans le résultat de
 *  l'exécution (`BANC_SORTIE`), créé au besoin. Un worker à la fois
 *  (`maxInstances: 1`) : pas de concurrence sur le fichier. */
export function verser(cle, meta, echantillons) {
  const chemin = process.env.BANC_SORTIE;
  if (!chemin) throw new Error("BANC_SORTIE absent : le banc se lance par scripts/banc.mjs");
  const r = existsSync(chemin)
    ? lireResultat(chemin)
    : nouveauResultat({ reference: process.env.BANC_REFERENCE ?? "inconnue", commit: process.env.BANC_COMMIT ?? "inconnu", plateforme: process.platform });
  ajouter(r, cle, meta, echantillons);
  ecrireResultat(r, chemin);
}

/** Consigne un scénario qui n'a pas pu se mesurer, avec sa raison. */
export function absent(cle, raison) {
  const chemin = process.env.BANC_SORTIE;
  if (!chemin) throw new Error("BANC_SORTIE absent : le banc se lance par scripts/banc.mjs");
  const r = existsSync(chemin)
    ? lireResultat(chemin)
    : nouveauResultat({ reference: process.env.BANC_REFERENCE ?? "inconnue", commit: process.env.BANC_COMMIT ?? "inconnu", plateforme: process.platform });
  r.absents.push(`${cle} : ${raison}`);
  ecrireResultat(r, chemin);
}
