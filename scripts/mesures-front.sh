#!/usr/bin/env bash
# Mesures sur la vraie application : remplacé le 27 novembre 2026 par le banc
# de mesures (scripts/banc.mjs, e2e/banc/), qui mesure le démarrage, la frappe,
# le débit du terminal, dix sessions mixtes et le bureau à distance, sous Linux
# comme sous Windows, et compare deux versions. Ce script garde l'ancien point
# d'entrée : une ligne de base de l'arbre courant, jeu rapide.
#
# Usage : scripts/mesures-front.sh [options de scripts/banc.mjs]
set -euo pipefail
cd "$(dirname "$0")/.."
exec node scripts/banc.mjs --jeu rapide --repetitions 1 "$@"
