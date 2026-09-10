#!/usr/bin/env bash
# Les paquets portés (rdp-sidecar/vendor, test-rdp-server/vendor) contre
# crates.io : dit lesquels ont une version amont plus récente que celle portée.
#
# Un paquet porté ne remonte pas tout seul : Dependabot ne voit que les
# dépendances déclarées, jamais un répertoire vendor/. Sans ce relevé, l'amont
# avance en silence et la fusion de ses correctifs sur les nôtres devient une
# opération de plus en plus lourde (constaté le 10 septembre 2026 : sept
# paquets portés, tous à la version amont du jour, mais rien ne l'aurait dit le
# jour où ce ne serait plus vrai). Joué chaque semaine par le workflow Qualité ;
# rougit dès qu'un paquet est en retard, pour que ce soit une décision et non un
# oubli. Réseau nécessaire (index crates.io).
set -uo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

en_retard=0
for manifeste in rdp-sidecar/vendor/*/Cargo.toml test-rdp-server/vendor/*/Cargo.toml; do
  [ -f "$manifeste" ] || continue
  nom="$(sed -n 's/^name *= *"\([^"]*\)".*/\1/p' "$manifeste" | head -1)"
  local_v="$(sed -n 's/^version *= *"\([^"]*\)".*/\1/p' "$manifeste" | head -1)"
  amont="$(cargo info "$nom" 2>/dev/null | sed -n 's/^version: *\([0-9][^ ]*\).*/\1/p' | head -1)"
  if [ -z "$amont" ]; then
    printf '  ? %-22s %-10s (crates.io injoignable ou paquet inconnu)\n' "$nom" "$local_v"
    continue
  fi
  # `cargo info` répond la version résolue ou, sans verrou, la plus récente ;
  # la mention « (latest X) » porte la plus récente quand elles diffèrent.
  derniere="$(cargo info "$nom" 2>/dev/null | sed -n 's/.*(latest \([0-9][^)]*\)).*/\1/p' | head -1)"
  [ -n "$derniere" ] && amont="$derniere"
  if [ "$amont" != "$local_v" ]; then
    printf '  ✗ %-22s porté %-10s amont %s\n' "$nom" "$local_v" "$amont"
    en_retard=1
  else
    printf '  ✓ %-22s %s\n' "$nom" "$local_v"
  fi
done

if [ "$en_retard" -ne 0 ]; then
  echo "✗ paquets portés : l'amont a avancé ; fusionner ses correctifs sur les nôtres (voir vendor/README.md), ou noter ici pourquoi on attend." >&2
  exit 1
fi
echo "✓ paquets portés : tous à la version amont courante"
