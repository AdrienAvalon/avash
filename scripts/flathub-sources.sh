#!/usr/bin/env bash
# Régénère les sources hors ligne du manifeste Flathub
# (packaging/flathub/*.json) à partir des fichiers de verrouillage du dépôt :
# Cargo.lock de l'espace de travail, rdp-sidecar/Cargo.lock et
# web/package-lock.json. À relancer à chaque montée de version ou de
# dépendance avant de soumettre à Flathub, puis commiter les JSON.
#
# Les générateurs viennent de flatpak-builder-tools (flatpak/flatpak-builder-tools
# sur GitHub), récupéré une fois dans le cache de l'utilisateur ; ils demandent
# Python 3 avec aiohttp et tomlkit (pacman : python-aiohttp python-tomlkit).
#
# Le générateur est épinglé sur un commit, et vérifié avant de s'exécuter.
# Trouvé par l'audit du 12 septembre 2026 (C-chaine-9) : le script clonait la
# branche principale, puis exécutait ce qu'il y trouvait ; sa sortie, l'URL et
# l'empreinte de chaque source que Flathub téléchargera, est commitée en JSON de
# plusieurs milliers de lignes où une empreinte réécrite passe inaperçue. Le
# commit ci-dessous est celui qui a produit les sources de la 0.12.2
# (30 août 2026). Pour le monter : lire le diff amont, changer le commit,
# régénérer, lire le diff des JSON. La sortie est ensuite confrontée aux
# verrous (scripts/tests/flathub-sources-coherentes-avec-les-verrous.sh).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTILS="${FLATPAK_BUILDER_TOOLS:-${XDG_CACHE_HOME:-$HOME/.cache}/flatpak-builder-tools}"
DEST="$ROOT/packaging/flathub"
COMMIT_OUTILS="1fc32195e3e60fe5c97f0af646dec7a99df5962b"

if [ ! -d "$OUTILS/.git" ]; then
  git init -q "$OUTILS"
  git -C "$OUTILS" remote add origin https://github.com/flatpak/flatpak-builder-tools.git
fi
if [ "$(git -C "$OUTILS" rev-parse HEAD 2>/dev/null || true)" != "$COMMIT_OUTILS" ]; then
  git -C "$OUTILS" fetch -q --depth 1 origin "$COMMIT_OUTILS"
  git -C "$OUTILS" checkout -q --detach FETCH_HEAD
fi
if [ "$(git -C "$OUTILS" rev-parse HEAD)" != "$COMMIT_OUTILS" ] \
   || [ -n "$(git -C "$OUTILS" status --porcelain --untracked-files=no)" ]; then
  echo "flatpak-builder-tools ($OUTILS) n'est pas le commit relu $COMMIT_OUTILS, ou il est modifié localement : rien n'est généré" >&2
  exit 1
fi

python3 "$OUTILS/cargo/flatpak-cargo-generator.py" "$ROOT/Cargo.lock" \
  -o "$DEST/cargo-sources.json"
python3 "$OUTILS/cargo/flatpak-cargo-generator.py" "$ROOT/rdp-sidecar/Cargo.lock" \
  -o "$DEST/cargo-sources-rdp.json"
( cd "$OUTILS/node" && python3 -m flatpak_node_generator npm \
    "$ROOT/web/package-lock.json" -o "$DEST/node-sources.json" )

ls -l "$DEST"/*.json
# Juste régénérées, les sources doivent couvrir exactement les verrous.
STRICT=1 "$ROOT/scripts/tests/flathub-sources-coherentes-avec-les-verrous.sh"
