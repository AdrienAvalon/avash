#!/usr/bin/env bash
# Contrôle reproductible : l'archive de mise à jour macOS (Avash.app.tar.gz)
# que fabrique scripts/archiver-app-macos.sh a la structure que Tauri
# produisait et que le greffon updater sait installer.
#
# Origine : audit du 12 septembre 2026 (C-chaine-1). Pour sortir la clé de
# signature du build, release.yml construit désormais avec
# createUpdaterArtifacts coupé : tauri-cli ne fabrique plus l'archive, c'est ce
# script qui le fait, et un autre job la signe. Le format est celui de
# tauri-bundler 2.9.4 (updater_bundle.rs, create_tar_from_src pour macOS) :
# `tar::Builder::append_dir_all("Avash.app", …)` sans suivre les liens, gzip au
# niveau par défaut. Le greffon (tauri-plugin-updater 2.11.0, install_inner
# macOS) retire le PREMIER composant de chaque chemin avant d'extraire, puis
# remplace l'application par le résultat : une archive en `./Avash.app/…`, un
# fichier AppleDouble `._*` ou un lien suivi casseraient l'installation, ou
# la signature du paquet .app.
#
# Le contrôle fabrique une fausse application (binaire exécutable, plist,
# cadre avec liens symboliques Versions/Current comme dans un vrai
# .framework), l'archive avec le vrai script, puis vérifie l'archive membre par
# membre et rejoue l'extraction du greffon.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

SCRIPT="scripts/archiver-app-macos.sh"
if [ ! -x "$SCRIPT" ]; then
  echo "  ✗ $SCRIPT absent ou non exécutable : personne ne fabrique Avash.app.tar.gz" >&2
  exit 1
fi

bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT
app="$bac/bundle/macos/Avash.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources" \
         "$app/Contents/Frameworks/Lib.framework/Versions/A"
printf '<plist/>\n' > "$app/Contents/Info.plist"
printf 'binaire' > "$app/Contents/MacOS/avash-ui"
printf 'sidecar' > "$app/Contents/MacOS/avash-rdp"
printf 'icone' > "$app/Contents/Resources/Avash.icns"
printf 'lib' > "$app/Contents/Frameworks/Lib.framework/Versions/A/Lib"
chmod 755 "$app/Contents/MacOS/avash-ui" "$app/Contents/MacOS/avash-rdp"
chmod 644 "$app/Contents/Info.plist" "$app/Contents/Resources/Avash.icns"
ln -s A "$app/Contents/Frameworks/Lib.framework/Versions/Current"
ln -s Versions/Current/Lib "$app/Contents/Frameworks/Lib.framework/Lib"

if ! "$SCRIPT" "$app" >/dev/null; then
  echo "  ✗ $SCRIPT a échoué sur une application valide" >&2
  exit 1
fi

python3 - "$app" "$app.tar.gz" <<'PY'
import os
import sys
import tarfile
import tempfile

app, archive = sys.argv[1], sys.argv[2]
echecs = []

if not os.path.isfile(archive):
    print(f"  ✗ archive absente : {archive} (attendue à côté de l'application)", file=sys.stderr)
    sys.exit(1)
with open(archive, "rb") as f:
    if f.read(2) != b"\x1f\x8b":
        echecs.append("l'archive n'est pas compressée par gzip (le greffon lit un GzDecoder)")

with tarfile.open(archive, "r:gz") as t:
    membres = t.getmembers()

if not membres or membres[0].name.rstrip("/") != "Avash.app" or not membres[0].isdir():
    echecs.append("le premier membre n'est pas le dossier Avash.app")
for m in membres:
    parties = m.name.rstrip("/").split("/")
    if parties[0] != "Avash.app":
        echecs.append(f"{m.name} : hors du dossier Avash.app (le greffon retire le premier composant)")
    if any(p in (".", "..", "") for p in parties) or m.name.startswith("/"):
        echecs.append(f"{m.name} : composant « . », « .. » ou chemin absolu")
    if any(p.startswith("._") for p in parties):
        echecs.append(f"{m.name} : fichier AppleDouble, il atterrirait dans l'application")

attendu = {}
for racine, dossiers, fichiers in os.walk(app):
    for nom in dossiers + fichiers:
        p = os.path.join(racine, nom)
        attendu["Avash.app/" + os.path.relpath(p, app)] = p
attendu["Avash.app"] = app
presents = {m.name.rstrip("/"): m for m in membres}
for nom in sorted(set(attendu) - set(presents)):
    echecs.append(f"{nom} : absent de l'archive")
for nom in sorted(set(presents) - set(attendu)):
    echecs.append(f"{nom} : en trop dans l'archive")

for nom, m in presents.items():
    source = attendu.get(nom)
    if source is None:
        continue
    if os.path.islink(source):
        if not m.issym() or m.linkname != os.readlink(source):
            echecs.append(f"{nom} : lien symbolique non conservé tel quel (suivi ou cible changée)")
    elif os.path.isfile(source):
        if not m.isfile():
            echecs.append(f"{nom} : n'est plus un fichier ordinaire")
        elif (m.mode & 0o777) != (os.stat(source).st_mode & 0o777):
            echecs.append(f"{nom} : droits {oct(m.mode & 0o777)} au lieu de {oct(os.stat(source).st_mode & 0o777)}")

# Extraction à la manière du greffon : premier composant retiré, le reste
# posé dans un dossier temporaire qui devient la nouvelle application.
with tempfile.TemporaryDirectory() as dest, tarfile.open(archive, "r:gz") as t:
    for m in t.getmembers():
        reste = "/".join(m.name.rstrip("/").split("/")[1:])
        cible = os.path.join(dest, reste) if reste else dest
        if m.isdir():
            os.makedirs(cible, exist_ok=True)
        elif m.issym():
            os.makedirs(os.path.dirname(cible), exist_ok=True)
            os.symlink(m.linkname, cible)
        else:
            os.makedirs(os.path.dirname(cible), exist_ok=True)
            with open(cible, "wb") as f:
                f.write(t.extractfile(m).read())
    for rel in ("Contents/MacOS/avash-ui", "Contents/MacOS/avash-rdp", "Contents/Info.plist"):
        if not os.path.isfile(os.path.join(dest, rel)):
            echecs.append(f"extraction du greffon : {rel} absent de l'application installée")

if echecs:
    for e in echecs:
        print(f"  ✗ {e}", file=sys.stderr)
    sys.exit(1)
print(f"  ✓ Avash.app.tar.gz : {len(membres)} membres sous Avash.app/, liens et droits conservés, installable par le greffon")
PY

# Le job macOS de release.yml doit appeler ce script et publier son archive.
if ! grep -q 'scripts/archiver-app-macos.sh' .github/workflows/release.yml; then
  echo "  ✗ release.yml n'appelle pas $SCRIPT : l'archive de mise à jour macOS ne serait plus produite" >&2
  exit 1
fi
