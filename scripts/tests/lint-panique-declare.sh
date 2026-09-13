#!/usr/bin/env bash
# Contrôle reproductible : les lints clippy `unwrap_used` et `expect_used` sont
# déclarés pour l'espace de travail (et hérités par ses deux membres) comme pour
# le sidecar RDP, qui est hors espace de travail.
#
# Trouvé le 13 septembre 2026 : l'audit du 12 avait retiré les unwrap et expect
# du code de production, écrit dans clippy.toml que `unwrap_used` « devient
# bloquant » et posé les `#![allow]` des tests et des exemples, mais les deux
# lints n'avaient jamais été déclarés. Rien ne rougissait : il restait cinq
# `expect` en production, et un `unwrap` ajouté le lendemain serait passé.
# Clippy ne peut pas signaler un lint absent ; ce script, si.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echec=0

# Lignes de la section TOML `[$2]` du fichier $1, jusqu'à la section suivante.
section() { # fichier nom_de_section
  awk -v s="[$2]" '$0 == s { dans = 1; next } /^\[/ { dans = 0 } dans' "$1"
}

exige_lint() { # fichier section lint
  if ! section "$1" "$2" | grep -qE "^[[:space:]]*$3[[:space:]]*=[[:space:]]*\"(warn|deny|forbid)\""; then
    echo "  ✗ $1 : [$2] ne déclare pas $3 (warn, deny ou forbid)" >&2
    echec=1
  fi
}

for lint in unwrap_used expect_used; do
  exige_lint Cargo.toml workspace.lints.clippy "$lint"
  exige_lint rdp-sidecar/Cargo.toml lints.clippy "$lint"
done

# Les lints de l'espace de travail ne valent que chez les membres qui les héritent.
for membre in crates/avash crates/avash-ui; do
  if ! section "$membre/Cargo.toml" lints | grep -qE '^[[:space:]]*workspace[[:space:]]*=[[:space:]]*true'; then
    echo "  ✗ $membre/Cargo.toml : pas de « [lints] workspace = true », les lints de l'espace de travail ne s'y appliquent pas" >&2
    echec=1
  fi
done

if [ "$echec" -ne 0 ]; then
  echo "✗ lint-panique-declare : unwrap et expect ne sont plus refusés partout en production." >&2
  exit 1
fi
echo "✓ lint-panique-declare : unwrap_used et expect_used déclarés (espace de travail, ses deux membres, sidecar)"
