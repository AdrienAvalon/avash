#!/usr/bin/env bash
# Régénère les sources hors ligne du manifeste Flathub
# (packaging/flathub/*.json) à partir des fichiers de verrouillage du dépôt :
# Cargo.lock de l'espace de travail, rdp-sidecar/Cargo.lock et
# web/package-lock.json. À relancer à chaque montée de version ou de
# dépendance avant de soumettre à Flathub, puis commiter les JSON.
#
# Les générateurs viennent de flatpak-builder-tools (flatpak/flatpak-builder-tools
# sur GitHub), cloné une fois dans le cache de l'utilisateur ; ils demandent
# Python 3 avec aiohttp et tomlkit (pacman : python-aiohttp python-tomlkit).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTILS="${FLATPAK_BUILDER_TOOLS:-${XDG_CACHE_HOME:-$HOME/.cache}/flatpak-builder-tools}"
DEST="$ROOT/packaging/flathub"

if [ ! -d "$OUTILS" ]; then
  git clone --depth 1 https://github.com/flatpak/flatpak-builder-tools.git "$OUTILS"
fi

python3 "$OUTILS/cargo/flatpak-cargo-generator.py" "$ROOT/Cargo.lock" \
  -o "$DEST/cargo-sources.json"
python3 "$OUTILS/cargo/flatpak-cargo-generator.py" "$ROOT/rdp-sidecar/Cargo.lock" \
  -o "$DEST/cargo-sources-rdp.json"
( cd "$OUTILS/node" && python3 -m flatpak_node_generator npm \
    "$ROOT/web/package-lock.json" -o "$DEST/node-sources.json" )

ls -l "$DEST"/*.json
