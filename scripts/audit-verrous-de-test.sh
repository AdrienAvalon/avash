#!/usr/bin/env bash
# Audite les fichiers de verrouillage qui ne partent dans aucun binaire publié :
# serveurs de test, cibles de fuzzing, et verrous autonomes des paquets portés
# (ceux sous lesquels leurs propres tests sont joués).
#
# Trouvé le 22 novembre 2026 : `cargo audit` ne regardait que l'espace de
# travail et le processus RDP. Le Scorecard de l'OpenSSF, qui lit tous les
# verrous du dépôt, y comptait quarante-cinq avis, dont des versions de rustls
# et de russh corrigées depuis des mois dans ce qui est livré. Rien de publié
# n'était touché, mais « aucune alerte sans justification écrite » vaut pour
# tout le dépôt.
#
# cargo-audit lit sa liste d'exceptions dans ./.cargo/audit.toml : chaque audit
# part donc du dossier dont la liste s'applique (racine, rdp-sidecar,
# test-rdp-server). Appelé par check.sh et par les deux chaînes d'intégration.
#
# Usage : scripts/audit-verrous-de-test.sh [options de cargo audit]
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echecs=0
auditer() { # dossier d'où lancer, verrou (relatif à ce dossier)
  local dossier="$1" verrou="$2"
  if (cd "$dossier" && cargo audit --quiet --deny unsound --file "$verrou" "${OPTIONS[@]}"); then
    echo "  ✓ $dossier/$verrou"
  else
    echo "  ✗ $dossier/$verrou" >&2
    echecs=1
  fi
}
OPTIONS=("$@")

auditer . fuzz/Cargo.lock
auditer test-vnc-server Cargo.lock
auditer test-rdp-server Cargo.lock
auditer test-rdp-server vendor/ironrdp-server/Cargo.lock
for porte in rdp-sidecar/vendor/*/Cargo.lock; do
  auditer rdp-sidecar "${porte#rdp-sidecar/}"
done
exit "$echecs"
