#!/usr/bin/env bash
# `npm audit`, en réessayant quand le registre est indisponible.
#
# Le 2026-09-03, le job bout-en-bout de la chaîne GitLab est tombé sur
# « 503 Service Unavailable » du point d'audit de registry.npmjs.org, après
# sept minutes d'attente de npm, sans qu'aucune vulnérabilité soit en cause.
# Une panne passagère du registre ne dit rien du code : on réessaie, trois
# fois, avec des délais de réseau bornés pour que l'ensemble tienne en
# quelques minutes. Une vulnérabilité trouvée, elle, reste une erreur au
# premier essai : on ne réessaie que sur une erreur du point d'audit.
#
# La panne se reconnaît à la STRUCTURE de `npm audit --json`, jamais au texte.
# Trouvé par l'audit du 12 septembre 2026 (C-chaine-11) : le script cherchait
# ECONNRESET, « socket hang up »… dans toute la sortie, titres d'avis compris,
# et un avis dont le titre contenait l'un de ces mots était pris pour une
# panne, puis toléré en mode tolerer-registre. Désormais :
#   - un rapport (auditReportVersion, metadata.vulnerabilities) n'est jamais
#     une panne : on compte ses avis au niveau demandé et au-dessus ;
#   - une panne est un objet `error` SANS rapport, dont le code est absent
#     (point d'audit injoignable : npm 12 rend alors `message` et un `error`
#     vide, vérifié le 12 septembre 2026) ou réseau (ECONNRESET, E503…) ;
#   - tout le reste (ENOLOCK, sortie illisible) est une erreur, sans réessai.
# Garde : scripts/tests/npm-audit-avis-non-confondu-avec-panne.sh.
#
# Avis acceptés : scripts/npm-audit-acceptes.txt, jumeau de .cargo/audit.toml
# (ajouté le 2026-10-06 pour braces, GHSA-vfj7-8cjw-p6xm, sans correctif amont).
# Un paquet ne sort du compte que s'il n'est vulnérable QUE par des avis
# acceptés, propagation comprise. Garde : le même script de test.
#
# Usage : npm-audit.sh <niveau> [tolerer-registre]
#   niveau : low, moderate, high ou critical.
#   tolerer-registre : après trois essais, une panne du registre est un
#   avertissement, pas une erreur. Réservé à la suite bout en bout, dont les
#   dépendances ne tournent que sur la machine de test : le 2026-09-04, le
#   point d'audit a refusé sa requête (480 paquets, « Bad Request ») pendant
#   des heures, rougissant chaque pipeline et chaque PR Dependabot sans
#   qu'aucune faille soit en cause. Le front, périmètre de confiance réel,
#   n'a pas cette tolérance : sans registre, pas de verdict, donc échec.
# NPM_AUDIT_PAUSE : secondes entre deux essais (20 ; 0 pour la garde).
set -uo pipefail
niveau="${1:?niveau attendu : low, moderate, high ou critical}"
tolerer="${2:-}"
pause="${NPM_AUDIT_PAUSE:-20}"
case "$niveau" in
  low|moderate|high|critical) ;;
  *) echo "npm audit : niveau inconnu « $niveau » (low, moderate, high ou critical)" >&2; exit 2 ;;
esac

# Lit le JSON sur l'entrée ; première ligne : « avis <n> », « panne <motif> »,
# « erreur <code> » ou « illisible » ; lignes suivantes : résumé lisible.
lire() {
  node -e '
const niveaux = ["info", "low", "moderate", "high", "critical"];
const seuil = niveaux.indexOf(process.argv[1]);
let d;
try { d = JSON.parse(require("fs").readFileSync(0, "utf8")); } catch { console.log("illisible"); process.exit(0); }
if (d && d.auditReportVersion && d.metadata && d.metadata.vulnerabilities) {
  const v = d.metadata.vulnerabilities;
  // Gravité effective d’un paquet : la plus haute de ses avis NON acceptés et
  // de celles de ses dépendances vulnérables (via sous forme de nom). Point
  // fixe, croissant donc fini : sans avis accepté, on retrouve la gravité npm.
  const acceptes = new Set(process.argv[2].split(",").filter(Boolean));
  const ghsa = (x) => String(x.url || "").split("/").pop();
  const paquets = d.vulnerabilities || {};
  const eff = Object.fromEntries(Object.keys(paquets).map((k) => [k, -1]));
  const vus = new Map();
  for (let change = true; change; ) {
    change = false;
    for (const [nom, a] of Object.entries(paquets)) {
      let g = -1;
      for (const x of a.via || []) {
        if (typeof x === "object" && acceptes.has(ghsa(x))) vus.set(ghsa(x), x.title);
        else g = Math.max(g, typeof x === "object" ? niveaux.indexOf(x.severity) : (eff[x] ?? -1));
      }
      if (g > eff[nom]) { eff[nom] = g; change = true; }
    }
  }
  const retenus = Object.keys(paquets).filter((k) => eff[k] >= seuil);
  // Sans avis écarté, le compte reste celui du relevé npm, comme avant la
  // liste ; « écarté » : npm, qui ignore la liste, sort alors en erreur, et
  // c’est attendu.
  const n = vus.size ? retenus.length : niveaux.slice(seuil).reduce((s, k) => s + (v[k] || 0), 0);
  console.log(`avis ${n}${vus.size ? " écarté" : ""}`);
  console.log(`npm audit : ${n} avis au niveau ${process.argv[1]} ou au-dessus (relevé npm : ${niveaux.map((k) => `${k} ${v[k] || 0}`).join(", ")})`);
  for (const nom of vus.size ? retenus : Object.keys(paquets).filter((k) => niveaux.indexOf(paquets[k].severity) >= seuil)) {
    const a = paquets[nom];
    const titres = (a.via || []).filter((x) => typeof x === "object" && !acceptes.has(ghsa(x))).map((x) => x.title).join(" ; ");
    console.log(`  ${niveaux[vus.size ? eff[nom] : niveaux.indexOf(a.severity)]} : ${nom}${titres ? " (" + titres + ")" : ""}`);
  }
  for (const [id, titre] of vus) console.log(`  accepté : ${id} (${titre}), voir scripts/npm-audit-acceptes.txt`);
  for (const id of acceptes) if (!vus.has(id)) console.log(`  avis accepté absent de cet audit : ${id}, à retirer de la liste s’il ne sort plus d’aucun arbre`);
} else if (d && typeof d.error === "object" && d.error !== null) {
  const code = d.error.code || "";
  const reseau = /^(E\d{3}|ENOAUDIT|ECONNRESET|ECONNREFUSED|ETIMEDOUT|ESOCKETTIMEDOUT|ENOTFOUND|EAI_AGAIN|EPIPE|ENETUNREACH|EHOSTUNREACH)$/;
  if (!code || reseau.test(code)) {
    console.log(`panne ${code || "point d’audit injoignable"}`);
  } else {
    console.log(`erreur ${code}`);
  }
  console.log(`npm audit : ${d.message || d.error.summary || code}`);
} else {
  console.log("illisible");
}
' "$niveau" "$acceptes"
}

# Avis acceptés (identifiants GHSA en début de ligne), voir le fichier.
# NPM_AUDIT_ACCEPTES : autre liste, pour la garde.
liste="${NPM_AUDIT_ACCEPTES:-$(dirname "${BASH_SOURCE[0]}")/npm-audit-acceptes.txt}"
acceptes="$(grep -oE '^GHSA(-[0-9a-z]{4}){3}' "$liste" | paste -sd, -)" || acceptes=""

journal="$(mktemp)"
trap 'rm -f "$journal"' EXIT
for essai in 1 2 3; do
  sortie=$(npm audit --json --audit-level="$niveau" --fetch-timeout=60000 --fetch-retries=1 2>"$journal")
  code=$?
  lecture="$(printf '%s' "$sortie" | lire)"
  verdict="$(head -n1 <<<"$lecture")"
  resume="$(tail -n +2 <<<"$lecture")"
  case "$verdict" in
    "avis 0"|"avis 0 écarté")
      printf '%s\n' "$resume"
      if [ "$code" -ne 0 ] && [ "$verdict" = "avis 0" ]; then
        echo "npm audit : code $code sans avis au niveau demandé ni panne reconnue" >&2
        cat "$journal" >&2
        exit 1
      fi
      exit 0 ;;
    avis\ *)
      printf '%s\n' "$resume" >&2
      exit 1 ;;
    panne\ *)
      printf '%s\n' "$resume" >&2
      if [ "$essai" -lt 3 ]; then
        echo "npm audit : registre indisponible (${verdict#panne }, essai $essai sur 3), nouvel essai dans $pause s" >&2
        sleep "$pause"
      fi
      continue ;;
    *)
      echo "npm audit : ${verdict}, pas une panne du registre (code $code)" >&2
      printf '%s\n' "$sortie" | head -40 >&2
      cat "$journal" >&2
      exit 1 ;;
  esac
done
if [ "$tolerer" = "tolerer-registre" ]; then
  echo "npm audit : registre indisponible après trois essais ; audit non rendu, toléré pour cet arbre (outillage de test)" >&2
  exit 0
fi
echo "npm audit : registre indisponible après trois essais, audit impossible" >&2
exit 1
