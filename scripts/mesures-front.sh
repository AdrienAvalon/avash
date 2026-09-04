#!/usr/bin/env bash
# Mesure le front sur la vraie application, avec le harnais de la suite bout
# en bout : démarrage (chargement et exécution du paquet JavaScript) et
# latence à la frappe sur la session SSH locale. Les chiffres sortent sur la
# console et dans e2e/mesures/resultat.json ; ils se reportent dans
# docs/feuille-de-route.md (axe 3 et tableau des indicateurs).
#
# Usage : scripts/mesures-front.sh
#
# À lancer après un build release, machine au repos : une compilation en
# parallèle fausse tout ce qui suit.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -x target/release/avash-ui ] || {
  echo "target/release/avash-ui absent : cargo build --release -p avash-ui d'abord" >&2
  exit 1
}
[ -x test-rdp-server/target/release/test-rdp-server ] || {
  echo "test-rdp-server absent : cargo build --release --manifest-path test-rdp-server/Cargo.toml d'abord" >&2
  exit 1
}
charge="$(cut -d' ' -f1 /proc/loadavg 2>/dev/null || echo 0)"
if [ "${charge%%.*}" -ge 4 ]; then
  echo "charge moyenne à $charge : la machine n'est pas au repos, les chiffres seraient faux" >&2
  exit 1
fi
cd e2e
xvfb-run -a -s "-screen 0 1440x900x24" npx wdio run wdio.mesures.conf.js
