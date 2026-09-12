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
  const n = niveaux.slice(seuil).reduce((s, k) => s + (v[k] || 0), 0);
  console.log(`avis ${n}`);
  console.log(`npm audit : ${n} avis au niveau ${process.argv[1]} ou au-dessus (relevé : ${niveaux.map((k) => `${k} ${v[k] || 0}`).join(", ")})`);
  for (const [nom, a] of Object.entries(d.vulnerabilities || {})) {
    if (niveaux.indexOf(a.severity) >= seuil) {
      const titres = (a.via || []).filter((x) => typeof x === "object").map((x) => x.title).join(" ; ");
      console.log(`  ${a.severity} : ${nom}${titres ? " (" + titres + ")" : ""}`);
    }
  }
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
' "$niveau"
}

journal="$(mktemp)"
trap 'rm -f "$journal"' EXIT
for essai in 1 2 3; do
  sortie=$(npm audit --json --audit-level="$niveau" --fetch-timeout=60000 --fetch-retries=1 2>"$journal")
  code=$?
  lecture="$(printf '%s' "$sortie" | lire)"
  verdict="$(head -n1 <<<"$lecture")"
  resume="$(tail -n +2 <<<"$lecture")"
  case "$verdict" in
    "avis 0")
      printf '%s\n' "$resume"
      if [ "$code" -ne 0 ]; then
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
