#!/usr/bin/env bash
# Compte les tests réellement exécutés par les paquets IronRDP portés.
#
# Ces paquets ont porté « test = false » pendant tout un temps, hérité du dépôt
# amont : les commandes de vérification s'exécutaient sans rien lancer, et les
# tests couvrant nos correctifs passaient pour verts sans jamais tourner. Une
# commande qui réussit sans rien faire est pire qu'une commande absente.
#
# Chaque paquet est éprouvé depuis SON répertoire, et non par `cargo test -p`
# depuis le processus RDP : `ironrdp-pdu` a des dépendances de développement et
# n'appartient pas à cet espace de travail, ce que cargo refuse en silence
# quand la commande vient de l'extérieur.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/vendor"
for p in ironrdp-session ironrdp-connector ironrdp-pdu ironrdp-graphics ironrdp-rdpdr ironrdp-svc vnc-rs; do
  # Trouvé par l'audit du 8 septembre 2026 : compter directement dans le tube
  # (`n=$(cargo test | grep | awk)`) engloutissait toute la sortie de cargo, et
  # sur un test porté en échec (cargo sort 101) ou une compilation cassée,
  # pipefail + set -e arrêtaient le script AVANT le moindre affichage : la porte
  # rougissait sans dire quel paquet ni quel test. On sépare donc exécution et
  # comptage : on capture la sortie, on l'imprime sur échec, puis on compte.
  if ! sortie=$(cd "$p" && cargo test 2>&1); then
    printf '%s\n' "$sortie" | tail -60 >&2
    echo "échec des tests de $p" >&2
    exit 1
  fi
  n=$(printf '%s\n' "$sortie" | grep -oP '^test result: ok\. \K\d+' \
      | awk '{s+=$1} END {print s+0}')
  [ "$n" -ge 1 ] || { echo "aucun test exécuté pour $p" >&2; exit 1; }
  echo "  $p : $n tests"
done
