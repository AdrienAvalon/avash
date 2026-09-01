#!/usr/bin/env bash
# Compte les tests réellement exécutés par les paquets IronRDP portés.
#
# Ces paquets ont porté « test = false » pendant tout un temps, hérité du dépôt
# amont : les commandes de vérification s'exécutaient sans rien lancer, et les
# tests couvrant nos correctifs passaient pour verts sans jamais tourner. Une
# commande qui réussit sans rien faire est pire qu'une commande absente.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
for p in ironrdp-session ironrdp-connector; do
  n=$(cargo test -p "$p" 2>&1 | grep -oP '^test result: ok\. \K\d+' \
      | awk '{s+=$1} END {print s+0}')
  [ "$n" -ge 1 ] || { echo "aucun test exécuté pour $p" >&2; exit 1; }
done
