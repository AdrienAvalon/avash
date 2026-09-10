#!/usr/bin/env bash
# Balaie les répertoires `target` du dépôt : chacun ne garde que les unités
# que ses commandes de vérification utilisent encore (scripts/balayer-target.sh).
#
#   scripts/balayer-cibles.sh [--simuler] [workspace|sidecar|serveurs]...
#
# Sans argument, les trois. Les listes de commandes ci-dessous sont CELLES des
# jobs de .gitlab-ci.yml (rust : workspace ; processus-rdp : sidecar et
# serveurs) : une commande absente d'ici verrait ses artefacts retirés à
# chaque passage puis recompilés au suivant, ce qui ne casse rien mais coûte.
# Sur le poste, à jouer quand `target` prend trop de place (47 Go retirés du
# workspace et 11 du sidecar le 10 septembre 2026) ; les unités d'une commande
# jouée seulement ici (cargo check de check.sh, --features webdriver de la
# suite embarquée) reviennent au passage suivant.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

options=()
if [ "${1:-}" = "--simuler" ]; then options=(--simuler); shift; fi
[ $# -gt 0 ] || set -- workspace sidecar serveurs

portes_sidecar=()
for p in ironrdp-session ironrdp-connector ironrdp-pdu ironrdp-graphics ironrdp-rdpdr ironrdp-svc vnc-rs; do
  portes_sidecar+=("rdp-sidecar/vendor/$p" "test --no-run")
done

for quoi in "$@"; do
  case "$quoi" in
    workspace)
      scripts/balayer-target.sh "${options[@]}" target \
        . "clippy --workspace --all-targets -- -D warnings" \
        . "clippy --workspace --release -- -D warnings" \
        . "test --workspace --all-targets --no-run" \
        . "build --release -p avash-ui" ;;
    sidecar)
      scripts/balayer-target.sh "${options[@]}" rdp-sidecar/target \
        rdp-sidecar "clippy --all-targets -- -D warnings" \
        rdp-sidecar "test --no-run" \
        rdp-sidecar "build --release"
      # Les paquets portés partagent rdp-sidecar/target/portes (verifier-portes.sh).
      scripts/balayer-target.sh "${options[@]}" rdp-sidecar/target/portes "${portes_sidecar[@]}" ;;
    serveurs)
      for s in test-rdp-server test-vnc-server; do
        scripts/balayer-target.sh "${options[@]}" "$s/target" \
          "$s" "clippy --all-targets -- -D warnings" \
          "$s" "test --no-run" \
          "$s" "build --release"
      done
      scripts/balayer-target.sh "${options[@]}" test-rdp-server/target/portes \
        test-rdp-server/vendor/ironrdp-server "test --no-run" ;;
    *) echo "cible inconnue : $quoi (workspace, sidecar ou serveurs)" >&2; exit 2 ;;
  esac
done
