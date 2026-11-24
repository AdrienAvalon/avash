#!/usr/bin/env bash
# Contrôle reproductible : les images des README ne dépendent d'aucun service
# qui interroge une API à chaque affichage, et le badge de version dit la
# version du dépôt.
#
# Vu le 24 novembre 2026 : les badges de version, winget, téléchargements et
# OpenSSF Scorecard s'affichaient en image cassée sur GitHub. Le mandataire
# d'images de GitHub abandonne au bout de quatre secondes, et shields.io
# mettait plus longtemps à interroger ses sources (les badges d'autres dépôts
# échouaient de même) ; le graphique de star-history, lui, était refusé (403).
# Les badges statiques, que shields.io dessine sans rien interroger, passaient
# tous. Depuis, les README n'emploient que ceux-là et les badges d'état des
# workflows servis par github.com ; la version se reporte à chaque publication
# (RELEASE.md, étape 1) et ce contrôle le vérifie.
set -uo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echecs=0
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"

for f in README.md README.en.md; do
  # Toute image distante hors des trois formes admises.
  dynamiques="$(grep -oE '(!\[[^]]*\]\(|<img[^>]*src=")https?://[^)" ]+' "$f" \
    | grep -oE 'https?://.*' \
    | grep -vE '^https://img\.shields\.io/badge/' \
    | grep -vE '^https://github\.com/AdrienAvalon/avash/actions/workflows/[a-z-]+\.yml/badge\.svg' || true)"
  if [ -n "$dynamiques" ]; then
    echo "  ✗ $f : image servie par un service qui interroge une API à l'affichage :" >&2
    sed 's/^/      /' <<<"$dynamiques" >&2
    echo "      employer un badge statique (img.shields.io/badge/...) ou une image du dépôt" >&2
    echecs=1
  fi
  if ! grep -qF "img.shields.io/badge/version-${version}-" "$f"; then
    echo "  ✗ $f : le badge de version ne dit pas ${version} (version de Cargo.toml)" >&2
    echecs=1
  fi
done

[ "$echecs" -eq 0 ] || exit 1
echo "  ✓ README : badges statiques, version ${version}"
