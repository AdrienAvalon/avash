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
for p in ironrdp-session ironrdp-connector ironrdp-pdu ironrdp-graphics ironrdp-rdpdr vnc-rs; do
  n=$(cd "$p" && cargo test 2>&1 | grep -oP '^test result: ok\. \K\d+' \
      | awk '{s+=$1} END {print s+0}')
  [ "$n" -ge 1 ] || { echo "aucun test exécuté pour $p" >&2; exit 1; }
  echo "  $p : $n tests"
done
