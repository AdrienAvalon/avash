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
( cd "$ROOT/rdp-sidecar" && cargo build --release )
mkdir -p "$UI/binaries"
cp -v "$ROOT/rdp-sidecar/target/release/avash-rdp" "$UI/binaries/avash-rdp-$TRIPLE"

# 2) Build des bundles pour la plateforme courante.
step "Build des bundles Tauri"
if ! cargo tauri --version >/dev/null 2>&1; then
  echo "cargo-tauri absent. Installe-le : cargo install tauri-cli --version '^2.0' --locked" >&2
  exit 1
fi

# La configuration demande des artefacts de mise à jour : Tauri s'arrête alors
# s'il ne trouve pas la clé de signature. Elle vit hors du dépôt, chez le
# mainteneur. Sans elle, on construit quand même — un binaire non signé reste
# utilisable en local, il ne peut simplement pas servir de mise à jour.
CLE_MAJ="${AVASH_UPDATER_KEY:-$HOME/.config/avash-release/updater.key}"
if [ -f "$CLE_MAJ" ]; then
  TAURI_SIGNING_PRIVATE_KEY="$(cat "$CLE_MAJ")"
  export TAURI_SIGNING_PRIVATE_KEY
  export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD-}"
  echo "  clé de signature : $CLE_MAJ"
else
  echo "  ⚠ clé de signature absente ($CLE_MAJ) : artefacts non signés" >&2
fi
# NO_STRIP : le strip embarqué par linuxdeploy ne gère pas .relr.dyn (libs récentes).
( cd "$UI" && NO_STRIP=1 cargo tauri build )

# 3) Rassembler les artefacts dans dist-release/.
step "Collecte des artefacts"
rm -rf "$OUT"; mkdir -p "$OUT"
BUNDLE="$ROOT/target/release/bundle"
found=0
while IFS= read -r -d '' f; do
  cp -v "$f" "$OUT/"; found=1
done < <(find "$BUNDLE" -type f \( -name '*.AppImage' -o -name '*-setup.exe' -o -name '*.msi' \) -print0 2>/dev/null)
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
