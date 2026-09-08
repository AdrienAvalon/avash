#!/usr/bin/env bash
# Contrôle reproductible : la procédure de publication de RELEASE.md est
# complète et cohérente avec les sources qui font foi (le workflow Release, le
# metainfo AppStream, les builds --frozen des paquets).
#
# Trouvé par l'audit du 8 septembre 2026 : l'étape 1 de RELEASE.md n'énumérait
# que les fichiers qui DÉCLARENT la version et oubliait deux choses qu'un tag
# emporte pourtant, si bien qu'un mainteneur suivant la liste à la lettre pour
# la 0.9.3 produisait un paquet cassé :
#   - l'entrée `<release>` de `packaging/dev.avash.app.metainfo.xml`, que l'AUR
#     et Flathub embarquent depuis l'archive du tag : sans elle la logithèque
#     (GNOME Logiciels, KDE Discover) affiche la version précédente comme la
#     plus récente (`appstreamcli` ne s'en plaint pas, la logithèque si) ;
#   - les deux `Cargo.lock`, alors que le PKGBUILD (AUR) et le manifeste Flathub
#     construisent en `--frozen` : un verrou resté sur l'ancienne version fait
#     échouer `makepkg` sur « the lock file needs to be updated but --frozen was
#     passed » (la CI, en `cargo build` simple, régénère le verrou sans le dire).
# Par ailleurs l'étape de publication disait « les deux plateformes » quand le
# workflow en construit trois, et le tableau d'artefacts n'avait aucune ligne
# macOS alors que la release produit un `.dmg` et une archive `.app.tar.gz`.
#
# Le contrôle exige que RELEASE.md reste accordé à ces sources ; il rougit
# contre l'ancien état et verdit une fois la procédure complétée.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echecs=0
release_md="RELEASE.md"
workflow="$(cat .github/workflows/release.yml)"
metainfo="packaging/dev.avash.app.metainfo.xml"

# --- 1. Le tableau d'artefacts documente macOS si la release en construit ---
# Vérité : le workflow a une entrée de matrice macOS qui empaquette un .dmg.
if grep -qE '^\s*-\s*os:\s*macos' .github/workflows/release.yml; then
  if ! grep -qiE '^\|\s*macOS\s*\|' "$release_md"; then
    echo "  ✗ $release_md : le workflow construit macOS mais le tableau n'a aucune ligne macOS" >&2
    echecs=1
  fi
  if ! grep -qi '\.dmg' "$release_md"; then
    echo "  ✗ $release_md : l'artefact macOS .dmg n'est pas documenté" >&2
    echecs=1
  fi
fi

# --- 2. Le nombre de plateformes annoncé colle à la matrice du workflow ---
# Vérité : le nombre d'entrées `- os:` de la matrice de build.
nb_plateformes="$(grep -cE '^\s*-\s*os:' .github/workflows/release.yml)"
declare -A mot=( [2]="deux" [3]="trois" [4]="quatre" )
attendu="${mot[$nb_plateformes]:-}"
if [[ -n "$attendu" ]]; then
  # On ne tolère aucun autre compte de plateformes dans la phrase de publication.
  for faux in deux trois quatre; do
    [[ "$faux" == "$attendu" ]] && continue
    if grep -qE "les $faux plateformes" "$release_md"; then
      echo "  ✗ $release_md : « les $faux plateformes » alors que le workflow en construit $nb_plateformes ($attendu)" >&2
      echecs=1
    fi
  done
  if ! grep -qE "les $attendu plateformes" "$release_md"; then
    echo "  ✗ $release_md : la publication devrait parler de « les $attendu plateformes » ($nb_plateformes dans la matrice)" >&2
    echecs=1
  fi
fi

# --- 3. L'entrée <release> du metainfo est rappelée dans la procédure ---
# Vérité : le metainfo tient un bloc <releases> versionné, embarqué par le tag.
if grep -q '<releases>' "$metainfo"; then
  if ! grep -q 'metainfo' "$release_md"; then
    echo "  ✗ $release_md : rien ne dit d'ajouter l'entrée <release> du metainfo (logithèque figée à la version précédente)" >&2
    echecs=1
  fi
fi

# --- 4. Le rattrapage des Cargo.lock est rappelé si les paquets buildent --frozen ---
# Vérité : PKGBUILD (AUR) et manifeste Flathub construisent en --frozen.
if grep -q -- '--frozen' packaging/aur/avash/PKGBUILD \
   || grep -q -- '--frozen' packaging/flathub/io.github.AdrienAvalon.avash.yml; then
  if ! grep -q 'Cargo.lock' "$release_md"; then
    echo "  ✗ $release_md : rien ne dit de commiter les Cargo.lock, alors que les paquets buildent en --frozen" >&2
    echecs=1
  fi
fi

if [[ "$echecs" -ne 0 ]]; then
  echo "  → compléter la procédure de RELEASE.md (voir en-tête du contrôle)" >&2
  exit 1
fi

echo "  ✓ RELEASE.md : tableau, plateformes, metainfo et Cargo.lock accordés au workflow et aux paquets"
