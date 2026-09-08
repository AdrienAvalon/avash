#!/usr/bin/env bash
# Contrôle reproductible : la version de l'espace de travail est bien HÉRITÉE
# par les crates membres, et les emplacements de version réellement distincts
# affichent tous le même numéro.
#
# Trouvé par l'audit du 8 septembre 2026 : le [workspace.package] de Cargo.toml
# se présentait comme « un seul endroit à modifier pour une release », mais
# crates/avash/Cargo.toml et crates/avash-ui/Cargo.toml codaient leur `version`
# en dur au lieu de `version.workspace = true`. Bumper la seule ligne du
# workspace ne changeait donc rien aux binaires : la pastille de version et le
# manifeste de mise à jour pouvaient se contredire.
#
# Deux exigences :
#   1. Chaque crate DE L'ESPACE DE TRAVAIL hérite version/edition/license du
#      workspace (`*.workspace = true`) et ne code plus ces champs en dur — sans
#      quoi l'héritage annoncé est un mensonge.
#   2. Les emplacements de version réellement distincts (workspace, sidecar hors
#      workspace, tauri.conf.json, web/package.json) portent le même numéro.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echecs=0

# --- Version déclarée dans [workspace.package] de la racine ---
version_ws="$(awk '
  /^\[workspace\.package\]/ { dans = 1; next }
  /^\[/ { dans = 0 }
  dans && /^version[[:space:]]*=/ {
    gsub(/[" ]/, "", $0); split($0, a, "="); print a[2]; exit
  }' Cargo.toml)"
if [[ -z "$version_ws" ]]; then
  echo "  ✗ pas de version dans [workspace.package] de Cargo.toml" >&2
  exit 1
fi

# --- Les crates membres héritent-elles, au lieu de coder en dur ? ---
for crate in crates/avash/Cargo.toml crates/avash-ui/Cargo.toml; do
  for champ in version edition license; do
    if ! grep -qE "^[[:space:]]*${champ}\.workspace[[:space:]]*=[[:space:]]*true" "$crate"; then
      echo "  ✗ $crate : $champ n'hérite pas du workspace (${champ}.workspace = true manquant)" >&2
      echecs=1
    fi
    # Un champ codé en dur (ex. `version = "0.9.2"`) court-circuite l'héritage.
    if grep -qE "^[[:space:]]*${champ}[[:space:]]*=[[:space:]]*\"" "$crate"; then
      echo "  ✗ $crate : $champ codé en dur, l'héritage du workspace est mort" >&2
      echecs=1
    fi
  done
done

# --- Cohérence des emplacements réellement distincts ---
# Le sidecar est hors espace de travail (Cargo.toml:exclude) : il garde sa
# propre version en dur, à tenir alignée à la main.
version_sidecar="$(awk '
  /^\[package\]/ { dans = 1; next }
  /^\[/ { dans = 0 }
  dans && /^version[[:space:]]*=/ {
    gsub(/[" ]/, "", $0); split($0, a, "="); print a[2]; exit
  }' rdp-sidecar/Cargo.toml)"
version_tauri="$(grep -oE '"version"[[:space:]]*:[[:space:]]*"[^"]+"' crates/avash-ui/tauri.conf.json | head -n1 | grep -oE '[0-9][^"]*')"
version_web="$(grep -oE '"version"[[:space:]]*:[[:space:]]*"[^"]+"' web/package.json | head -n1 | grep -oE '[0-9][^"]*')"

for couple in "rdp-sidecar/Cargo.toml:$version_sidecar" \
              "crates/avash-ui/tauri.conf.json:$version_tauri" \
              "web/package.json:$version_web"; do
  fichier="${couple%%:*}"
  valeur="${couple#*:}"
  if [[ "$valeur" != "$version_ws" ]]; then
    echo "  ✗ $fichier annonce ${valeur:-?}, le workspace est en $version_ws" >&2
    echecs=1
  fi
done

if [[ "$echecs" -ne 0 ]]; then
  echo "  → aligner les versions et rétablir l'héritage du workspace" >&2
  exit 1
fi

echo "  ✓ version $version_ws héritée par les crates membres et alignée (sidecar, tauri, web)"
