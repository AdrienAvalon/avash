#!/usr/bin/env bash
# Contrôle reproductible : sous une cible Windows, scripts/release.sh dépose le
# sidecar RDP sous le nom attendu par Tauri (externalBin), avash-rdp.exe.
#
# Trouvé par l'audit du 7 septembre 2026 : l'en-tête du script (« à lancer SUR
# Windows ») et RELEASE.md §2 présentent release.sh comme le point d'entrée du
# build Windows, mais la copie du sidecar ciblait `avash-rdp` sans extension.
# Sous Windows le binaire s'appelle `avash-rdp.exe` et Tauri attend
# `binaries/avash-rdp-x86_64-pc-windows-msvc.exe` (cf. rdp.rs, EXE_SUFFIX, et
# release.yml qui ajoute bien l'extension) : `cp` échouait alors sur `set -e`,
# arrêtant le build avant `cargo tauri build`.
#
# Ce test extrait la VRAIE ligne de copie du script (du calcul de TRIPLE au
# `cp` du sidecar) et l'exécute dans un bac à sable, avec `rustc` et `cargo`
# simulés, pour deux cibles : Windows (source .exe, destination attendue .exe)
# et Linux (source et destination sans extension). Il ne dépend d'aucun binaire
# construit.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

RELEASE="scripts/release.sh"

# Bloc réel du script : de l'affectation de TRIPLE à la copie du sidecar.
BLOC="$(awk '/^TRIPLE=/{f=1} f{print} /^cp -v.*binaries\/avash-rdp/{exit}' "$RELEASE")"
if ! printf '%s\n' "$BLOC" | grep -q 'cp -v'; then
  echo "  ✗ impossible d'extraire la ligne de copie du sidecar de $RELEASE" >&2
  exit 1
fi

essayer_cible() {
  # $1 = triple simulé (retourné par le faux rustc)
  # $2 = suffixe du binaire source à créer ("" ou ".exe")
  # $3 = nom de destination attendu sous binaries/
  local triple="$1" suffixe="$2" attendu="$3"
  local sb stub
  sb="$(mktemp -d)"
  stub="$sb/stub"
  mkdir -p "$stub" "$sb/rdp-sidecar/target/release" "$sb/crates/avash-ui"
  # Le binaire du sidecar tel qu'il existe VRAIMENT sur cette plateforme.
  : > "$sb/rdp-sidecar/target/release/avash-rdp$suffixe"

  # rustc simulé : n'imite que `-vV` (la ligne `host:` lue par le script).
  printf '#!/usr/bin/env bash\necho "host: %s"\n' "$triple" > "$stub/rustc"
  # cargo simulé : le build du sidecar est déjà représenté par le fichier posé.
  printf '#!/usr/bin/env bash\nexit 0\n' > "$stub/cargo"
  chmod +x "$stub/rustc" "$stub/cargo"

  if ! env ROOT="$sb" UI="$sb/crates/avash-ui" PATH="$stub:$PATH" \
        bash -euo pipefail -c "$BLOC" >/dev/null 2>&1; then
    echo "  ✗ [$triple] la copie du sidecar a échoué (le build s'arrête là, set -e)" >&2
    rm -rf "$sb"; return 1
  fi
  if [ ! -f "$sb/crates/avash-ui/binaries/$attendu" ]; then
    echo "  ✗ [$triple] destination attendue absente : binaries/$attendu" >&2
    echo "    présent : $(ls "$sb/crates/avash-ui/binaries" 2>/dev/null || echo '(rien)')" >&2
    rm -rf "$sb"; return 1
  fi
  rm -rf "$sb"
}

echecs=0
# Windows : source avash-rdp.exe, destination avash-rdp-<triple>.exe.
essayer_cible "x86_64-pc-windows-msvc" ".exe" \
  "avash-rdp-x86_64-pc-windows-msvc.exe" || echecs=1
# Linux : pas d'extension, ni à la source ni à la destination (garde contre un
# correctif qui collerait .exe partout).
essayer_cible "x86_64-unknown-linux-gnu" "" \
  "avash-rdp-x86_64-unknown-linux-gnu" || echecs=1

if [ "$echecs" -ne 0 ]; then
  exit 1
fi
echo "  ✓ release.sh dépose le sidecar avec le bon suffixe (Windows .exe, Linux sans)"
