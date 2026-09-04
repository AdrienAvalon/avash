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
# Usage : npm-audit.sh <niveau> [tolerer-registre]
#   niveau : low, moderate, high ou critical.
#   tolerer-registre : après trois essais, une panne du registre est un
#   avertissement, pas une erreur. Réservé à la suite bout en bout, dont les
#   dépendances ne tournent que sur la machine de test : le 2026-09-04, le
#   point d'audit a refusé sa requête (480 paquets, « Bad Request ») pendant
#   des heures, rougissant chaque pipeline et chaque PR Dependabot sans
#   qu'aucune faille soit en cause. Le front, périmètre de confiance réel,
#   n'a pas cette tolérance : sans registre, pas de verdict, donc échec.
set -uo pipefail
niveau="${1:?niveau attendu : low, moderate, high ou critical}"
tolerer="${2:-}"
for essai in 1 2 3; do
  sortie=$(npm audit --audit-level="$niveau" --fetch-timeout=60000 --fetch-retries=1 2>&1)
  code=$?
  printf '%s\n' "$sortie"
  [ "$code" -eq 0 ] && exit 0
  if printf '%s' "$sortie" | grep -qiE 'audit endpoint returned an error|Service Unavailable|ECONNRESET|ECONNREFUSED|ETIMEDOUT|ENOTFOUND|EAI_AGAIN|socket hang up'; then
    if [ "$essai" -lt 3 ]; then
      echo "npm audit : registre indisponible (essai $essai sur 3), nouvel essai dans 20 s" >&2
      sleep 20
    fi
    continue
  fi
  exit "$code"
done
if [ "$tolerer" = "tolerer-registre" ]; then
  echo "npm audit : registre indisponible après trois essais ; audit non rendu, toléré pour cet arbre (outillage de test)" >&2
  exit 0
fi
echo "npm audit : registre indisponible après trois essais, audit impossible" >&2
exit 1
