#!/usr/bin/env bash
# Construit les binaires de distribution d'Avash et produit de quoi les
# vérifier (checksums SHA-256 + signature GPG détachée).
#
# Linux  : produit une AppImage (un seul fichier, copier-coller, exécutable).
# Windows: à lancer SUR Windows — produit l'installeur NSIS (.exe) et, si un
#          certificat Authenticode est configuré, le binaire est signé.
#
# La signature de code Windows (qui évite les alertes SmartScreen/antivirus)
# nécessite un certificat de l'utilisateur — voir RELEASE.md. Ce script ne
# fabrique aucune confiance : il automatise, il ne remplace pas le certificat.
#
# Usage : ./scripts/release.sh [--sign-gpg <KEYID>]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UI="$ROOT/crates/avash-ui"
OUT="$ROOT/dist-release"
GPG_KEY=""

while [ $# -gt 0 ]; do
  case "$1" in
    --sign-gpg) GPG_KEY="${2:?--sign-gpg exige un identifiant de clé}"; shift 2 ;;
    *) echo "Option inconnue : $1" >&2; exit 2 ;;
  esac
done

step() { printf '\n\033[1;36m▸ %s\033[0m\n' "$1"; }

# 1) Qualité avant tout : on ne release pas du code non validé.
step "Validation complète (check.sh)"
"$ROOT/check.sh"

# 1.5) Sidecar RDP (projet séparé, hors workspace) : construit et déposé sous
#      le nom attendu par Tauri (externalBin) pour être embarqué à côté de l'exe.
step "Build du sidecar RDP (avash-rdp)"
TRIPLE="$(rustc -vV | sed -n 's/host: //p')"
# Trouvé par l'audit du 7 septembre 2026 : sous Windows le binaire s'appelle
# avash-rdp.exe et Tauri (externalBin) attend binaries/avash-rdp-$TRIPLE.exe.
# Sans l'extension, `cp` échouait (set -e) et le build « sur Windows » annoncé
# en tête et dans RELEASE.md §2 s'arrêtait avant `cargo tauri build`. On aligne
# le suffixe sur la cible, comme EXE_SUFFIX côté cœur (rdp.rs).
EXT=""
case "$TRIPLE" in *windows*) EXT=.exe ;; esac
( cd "$ROOT/rdp-sidecar" && cargo build --locked --release )
mkdir -p "$UI/binaries"
cp -v "$ROOT/rdp-sidecar/target/release/avash-rdp$EXT" "$UI/binaries/avash-rdp-$TRIPLE$EXT"

# 2) Build des bundles pour la plateforme courante.
step "Build des bundles Tauri"
if ! cargo tauri --version >/dev/null 2>&1; then
  echo "cargo-tauri absent. Installe-le : cargo install tauri-cli --version 2.11.4 --locked" >&2
  exit 1
fi

# Construction SANS la clé de signature : createUpdaterArtifacts est coupé, Tauri
# ne signe rien et ne la réclame pas. Trouvé par l'audit du 12 septembre 2026
# (C-chaine-1) : ce script exportait la clé avant `cargo tauri build`, qui
# exécute vite, chaque build.rs et chaque macro procédurale ; n'importe
# laquelle de ces dépendances la lisait par un simple getenv. La signature
# vient après, par `signer sign`, seule commande à lire la clé.
# Les bundles d'une version précédente restent dans target/ : sans ce ménage,
# la collecte ramassait aussi les deb et rpm de la 0.7.2 à côté de la 0.8.0.
rm -rf "$ROOT/target/release/bundle"
# NO_STRIP : le strip embarqué par linuxdeploy ne gère pas .relr.dyn (libs récentes).
# `-- --locked` va à cargo : le binaire est fait du Cargo.lock commité.
( cd "$UI" && NO_STRIP=1 cargo tauri build --config '{"bundle":{"createUpdaterArtifacts":false}}' -- --locked )

# 2.5) Signature des artefacts de mise à jour, à côté de chacun (<fichier>.sig).
#      La clé vit hors du dépôt, chez le mainteneur ; elle est lue par son
#      chemin, par la seule commande qui signe. Sans elle, on continue : un
#      binaire non signé reste utilisable en local, il ne peut simplement pas
#      servir de mise à jour. Le mot de passe est toujours posé, fût-il vide :
#      absent, tauri-cli le demanderait au terminal.
step "Signature des artefacts de mise à jour"
CLE_MAJ="${AVASH_UPDATER_KEY:-$HOME/.config/avash-release/updater.key}"
if [ -f "$CLE_MAJ" ]; then
  echo "  clé de signature : $CLE_MAJ"
  while IFS= read -r -d '' f; do
    TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD-}" \
      cargo tauri signer sign -f "$CLE_MAJ" "$f" >/dev/null
    echo "  signé : $(basename "$f")"
  done < <(find "$ROOT/target/release/bundle" -type f \( -name '*.AppImage' -o -name '*.deb' -o -name '*.rpm' -o -name '*-setup.exe' \) -print0)
else
  echo "  ⚠ clé de signature absente ($CLE_MAJ) : artefacts non signés" >&2
fi

# 3) Rassembler les artefacts dans dist-release/.
step "Collecte des artefacts"
rm -rf "$OUT"; mkdir -p "$OUT"
BUNDLE="$ROOT/target/release/bundle"
found=0
while IFS= read -r -d '' f; do
  cp -v "$f" "$OUT/"; found=1
done < <(find "$BUNDLE" -type f \( -name '*.AppImage' -o -name '*.deb' -o -name '*.rpm' -o -name '*-setup.exe' -o -name '*.msi' \) -print0 2>/dev/null)
[ "$found" -eq 1 ] || { echo "Aucun artefact trouvé sous $BUNDLE" >&2; exit 1; }

# 4) Checksums SHA-256 : l'intégrité vérifiable sur n'importe quelle machine
#    (y compris une station d'analyse isolée).
step "Empreintes SHA-256"
( cd "$OUT" && sha256sum -- * > SHA256SUMS && cat SHA256SUMS )

# 5) Signature GPG détachée (authenticité). Facultative mais recommandée sous
#    Linux (l'AppImage n'a pas d'équivalent SmartScreen).
if [ -n "$GPG_KEY" ]; then
  step "Signature GPG ($GPG_KEY)"
  ( cd "$OUT" && gpg --local-user "$GPG_KEY" --armor --detach-sign --output SHA256SUMS.asc SHA256SUMS )
  echo "Vérification : gpg --verify SHA256SUMS.asc SHA256SUMS"
fi

step "Terminé"
echo "Artefacts prêts dans : $OUT"
ls -lh "$OUT"
