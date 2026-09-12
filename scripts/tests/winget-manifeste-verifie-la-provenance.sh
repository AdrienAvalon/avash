#!/usr/bin/env bash
# Contrôle reproductible : scripts/winget-manifeste.sh n'écrit l'empreinte de
# l'installeur qu'après avoir téléchargé ce fichier, vérifié son attestation de
# provenance et calculé l'empreinte lui-même.
#
# Trouvé par l'audit du 12 septembre 2026 (C-chaine-10) : l'empreinte publiée
# dans le manifeste winget était recopiée du SHA256SUMS de la release, fichier
# modifiable, ni signé ni attesté ; l'attestation Sigstore que produit
# release.yml n'était jamais consultée. Quelqu'un qui remplaçait l'installeur
# ET SHA256SUMS sur la page de release obtenait un manifeste cohérent avec son
# binaire, que le robot de winget-pkgs aurait installé chez tout le monde.
#
# Le vrai script est rejoué dans un bac à sable, avec un `gh` simulé :
#   A. attestation refusée : échec, aucun manifeste d'installeur écrit ;
#   B. attestation valide : l'empreinte écrite est celle du fichier téléchargé ;
#   C. attestation valide mais SHA256SUMS menteur : échec (les deux sources
#      d'empreinte doivent concorder).
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT
mkdir -p "$bac/scripts" "$bac/stub"
cp scripts/winget-manifeste.sh "$bac/scripts/"

cat > "$bac/stub/gh" <<'STUB'
#!/usr/bin/env bash
# `gh` simulé : journalise l'appel et imite les trois sous-commandes employées.
printf '%s\n' "$*" >> "$STUB_JOURNAL"
case "$1 $2" in
  "release download")
    dest="" motifs=()
    shift 2
    while [ $# -gt 0 ]; do
      case "$1" in
        -D|--dir) dest="$2"; shift 2 ;;
        -p|--pattern) motifs+=("$2"); shift 2 ;;
        *) shift ;;
      esac
    done
    for m in "${motifs[@]}"; do
      case "$m" in
        SHA256SUMS) printf '%s  Avash_%s_x64-setup.exe\n' "$STUB_SOMME" "$STUB_VERSION" > "$dest/SHA256SUMS" ;;
        *setup.exe) printf '%s' "$STUB_CONTENU" > "$dest/$m" ;;
      esac
    done ;;
  "attestation verify") exit "$STUB_ATTESTATION" ;;
  "release view") echo "2026-09-11" ;;
  *) echo "gh simulé : appel inattendu : $*" >&2; exit 99 ;;
esac
STUB
chmod +x "$bac/stub/gh"

v="9.9.9"
contenu="installeur authentique"
vraie="$(printf '%s' "$contenu" | sha256sum | cut -c1-64)"
installeur="$bac/packaging/winget/AdrienCros.Avash/$v/AdrienCros.Avash.installer.yaml"
echecs=0

jouer() { # <attestation 0|1> <somme dans SHA256SUMS>
  rm -rf "$bac/packaging"
  : > "$bac/journal"
  env PATH="$bac/stub:$PATH" STUB_JOURNAL="$bac/journal" STUB_ATTESTATION="$1" \
      STUB_SOMME="$2" STUB_VERSION="$v" STUB_CONTENU="$contenu" \
      bash "$bac/scripts/winget-manifeste.sh" "$v" >/dev/null 2>&1
}

# A. Attestation refusée.
if jouer 1 "$vraie"; then
  echo "  ✗ A : attestation refusée, le script a pourtant réussi" >&2; echecs=1
fi
if [ -f "$installeur" ]; then
  echo "  ✗ A : un manifeste d'installeur a été écrit sans provenance vérifiée" >&2; echecs=1
fi

# B. Attestation valide, empreintes concordantes.
if ! jouer 0 "$vraie"; then
  echo "  ✗ B : le script échoue sur une release saine" >&2; echecs=1
elif ! grep -q "InstallerSha256: ${vraie^^}\$" "$installeur"; then
  echo "  ✗ B : l'empreinte écrite n'est pas celle du fichier téléchargé" >&2; echecs=1
fi
if ! grep -q "attestation verify .*Avash_${v}_x64-setup.exe" "$bac/journal"; then
  echo "  ✗ B : aucun « gh attestation verify » sur l'installeur téléchargé" >&2; echecs=1
fi

# C. SHA256SUMS qui ment.
fausse="$(printf 'autre' | sha256sum | cut -c1-64)"
if jouer 0 "$fausse"; then
  echo "  ✗ C : SHA256SUMS contredit le fichier attesté, le script a pourtant réussi" >&2; echecs=1
fi

# L'attestation de la release doit couvrir SHA256SUMS lui-même.
if ! python3 - <<'PY'
import sys, yaml
wf = yaml.safe_load(open(".github/workflows/release.yml", encoding="utf-8"))
for e in wf["jobs"]["publier"]["steps"]:
    if str(e.get("uses", "")).startswith("actions/attest-build-provenance@"):
        sys.exit(0 if "SHA256SUMS" in str(e.get("with", {}).get("subject-path", "")).split() else 1)
sys.exit(1)
PY
then
  echo "  ✗ release.yml : SHA256SUMS n'est pas un sujet de l'attestation de provenance" >&2; echecs=1
fi

if [ "$echecs" -ne 0 ]; then
  exit 1
fi
echo "  ✓ winget-manifeste.sh vérifie la provenance et calcule l'empreinte de l'installeur lui-même"
