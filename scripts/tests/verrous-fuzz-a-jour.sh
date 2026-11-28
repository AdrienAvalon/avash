#!/usr/bin/env bash
# Contrôle reproductible : les verrous des cibles de fuzzing (`fuzz/` et
# `fuzz/rdp/`) concordent avec les manifestes qu'ils décrivent.
#
# Trouvé en préparant la 0.17.0 (28 novembre 2026) : les lots de l'audit du
# 27 novembre avaient ajouté keyring-core, socket2 et windows-sys au cœur, et
# bytes et socket2 au processus RDP, sans que les verrous de fuzzing suivent.
# Rien ne le voyait : ces cibles sont hors espace de travail, et le fuzzing de
# la CI se construit sans --locked. Seul le job « Sécurité avant publication »
# (--locked) aurait rougi, au moment même de publier. `cargo metadata --locked`
# refusait les deux verrous de main.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echecs=0
for manifeste in fuzz/Cargo.toml fuzz/rdp/Cargo.toml; do
  if ! sortie=$(cargo metadata --locked --format-version 1 --manifest-path "$manifeste" 2>&1 >/dev/null); then
    printf '%s\n' "$sortie" | tail -3 >&2
    echo "verrou en retard : ${manifeste%Cargo.toml}Cargo.lock (cargo update --manifest-path $manifeste, voir RELEASE.md)" >&2
    echecs=$((echecs + 1))
  fi
done
[ "$echecs" -eq 0 ] || exit 1
echo "verrous de fuzzing à jour"
