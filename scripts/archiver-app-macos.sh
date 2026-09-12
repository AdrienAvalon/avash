#!/usr/bin/env bash
# Fabrique l'archive de mise à jour macOS (<App>.app.tar.gz) à côté du paquet
# .app, dans le format que tauri-cli produisait lui-même.
#
# Pourquoi ce script : jusqu'à la 0.12.2, `cargo tauri build` fabriquait et
# signait cette archive, clé privée dans l'environnement de tout le build
# (vite, build.rs, macros procédurales). L'audit du 12 septembre 2026
# (C-chaine-1) a sorti la clé du build : release.yml construit avec
# createUpdaterArtifacts coupé, ce script archive, et le job `signer`, qui ne
# compile rien, signe.
#
# Format reproduit d'après tauri-bundler 2.9.4 (updater_bundle.rs,
# create_tar_from_src pour macOS) : `append_dir_all("<App>.app", …)` du crate
# tar, en-têtes GNU, liens symboliques conservés tels quels (les .framework en
# portent), gzip au niveau par défaut de flate2 (6). Le greffon updater
# (install_inner macOS) retire le premier composant de chaque chemin avant
# d'extraire : l'archive ne doit contenir que `<App>.app/…`, jamais `./`, ni
# fichier AppleDouble `._*` que le tar de macOS ajoute sans COPYFILE_DISABLE.
# D'où Python, qui n'écrit que ce qu'on lui donne. Garde :
# scripts/tests/release-archive-macos-structure.sh.
#
# Usage : scripts/archiver-app-macos.sh <chemin/vers/Avash.app>
set -euo pipefail
app="${1:?chemin du paquet .app attendu}"
app="${app%/}"
case "$app" in
  *.app) ;;
  *) echo "pas un paquet .app : $app" >&2; exit 1 ;;
esac
[ -d "$app" ] || { echo "paquet introuvable : $app" >&2; exit 1; }

python3 - "$app" "$app.tar.gz" <<'PY'
import os
import sys
import tarfile

app, archive = sys.argv[1], sys.argv[2]
with tarfile.open(archive, "w:gz", compresslevel=6, format=tarfile.GNU_FORMAT,
                  dereference=False) as t:
    t.add(app, arcname=os.path.basename(app), recursive=True)
PY
echo "$app.tar.gz"
