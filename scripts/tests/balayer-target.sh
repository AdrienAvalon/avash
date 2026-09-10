#!/usr/bin/env bash
# Contrôle reproductible de scripts/balayer-target.sh : ne retire d'un
# répertoire `target` que les unités que cargo n'utilise plus.
#
# Trouvé le 10 septembre 2026 sur l'exécuteur GitLab : `target` avait atteint
# 64 Go sur un poste de travail, l'archive de cache 19 Go, et le job rust
# passait 21 minutes à archiver pour 5 minutes de vérifications. Rien ne
# retire jamais un artefact d'un `target` : chaque montée de dépendance, chaque
# changement de version du projet, chaque toolchain laisse ses anciens rlib,
# binaires de test (460 Mo chacun pour avash-ui) et sorties de build.rs. Ni
# l'heure d'accès (le disque est monté noatime) ni `invoked.timestamp` (cargo
# stable ne le réécrit que pour une unité recompilée) ne disent ce qui sert
# encore : seule la sortie `--message-format=json` d'une commande cargo, à
# chaud, énumère les artefacts de TOUTES les unités du plan, fraîches
# comprises. Le script rejoue les commandes du job avec ce format (aucune
# compilation à chaud), collecte les hachages vivants, et retire le reste.
#
# Ce test remplace `cargo` par un faux qui énumère deux unités vivantes,
# construit un `target` de cinq unités, et exige : les vivantes intactes, les
# trois autres retirées (empreinte, deps, build, et une du profil release qui
# n'a pas été énumérée), le binaire sans hachage intact, les commandes rejouées
# telles quelles (`-- -D warnings` compris), et qu'une énumération vide ne
# retire RIEN (le faux cargo qui se tait rend une sortie non nulle).
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

SCRIPT="$PWD/scripts/balayer-target.sh"
[ -x "$SCRIPT" ] || { echo "  ✗ $SCRIPT absent ou non exécutable" >&2; exit 1; }

bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT
cible="$bac/target"
projet="$bac/projet"
mkdir -p "$projet"

# Le faux cargo : note ses arguments, puis énumère deux unités vivantes en JSON
# (un rlib et une sortie de build.rs), comme cargo le fait à chaud.
cat > "$bac/cargo" <<'FAUX'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$BAC/appels"
[ -n "${SILENCE:-}" ] && exit 0
case "$*" in *--message-format=json*) ;; *) echo "pas de --message-format=json" >&2; exit 1;; esac
c="$CARGO_TARGET_DIR"
echo "{\"reason\":\"compiler-artifact\",\"filenames\":[\"$c/debug/deps/libneuf-1111111111111111.rlib\",\"$c/debug/deps/libneuf-1111111111111111.rmeta\"],\"fresh\":true}"
echo "{\"reason\":\"build-script-executed\",\"out_dir\":\"$c/debug/build/neuf-2222222222222222/out\"}"
echo "{\"reason\":\"build-finished\",\"success\":true}"
FAUX
chmod +x "$bac/cargo"
export BAC="$bac"

unite() { # unite <profil> <nom-hachage> [fichiers de deps...]
  local profil="$1" nom="$2"; shift 2
  mkdir -p "$cible/$profil/.fingerprint/$nom" "$cible/$profil/deps" "$cible/$profil/build"
  echo x > "$cible/$profil/.fingerprint/$nom/invoked.timestamp"
  for f in "$@"; do echo contenu > "$cible/$profil/deps/$f"; done
}
peupler() {
  rm -rf "$cible"
  unite debug neuf-1111111111111111 libneuf-1111111111111111.rlib libneuf-1111111111111111.rmeta neuf-1111111111111111.d
  unite debug neuf-2222222222222222
  mkdir -p "$cible/debug/build/neuf-2222222222222222/out"; echo x > "$cible/debug/build/neuf-2222222222222222/out/g.rs"
  unite debug vieux-3333333333333333 libvieux-3333333333333333.rlib vieux-3333333333333333.d
  mkdir -p "$cible/debug/build/vieux-3333333333333333"; echo x > "$cible/debug/build/vieux-3333333333333333/build-script-build"
  unite debug mon-paquet-4444444444444444 libmon_paquet-4444444444444444.rlib
  unite release neuf-5555555555555555 libneuf-5555555555555555.rlib
  echo binaire > "$cible/debug/avash-ui"
}

echec=0
ko() { echo "  ✗ $*" >&2; echec=1; }
existe() { [ -e "$1" ] || ko "manque : ${1#$cible/}"; }
absent() { [ ! -e "$1" ] || ko "encore là : ${1#$cible/}"; }

# 1. Simulation : rien ne bouge, mais les unités périmées sont nommées.
peupler; : > "$bac/appels"
sortie="$(PATH="$bac:$PATH" "$SCRIPT" --simuler "$cible" "$projet" "clippy --all-targets -- -D warnings" "$projet" "test --no-run" 2>&1)" || ko "simulation : sortie non nulle"
for u in vieux-3333333333333333 mon-paquet-4444444444444444 neuf-5555555555555555; do
  grep -q "$u" <<<"$sortie" || ko "simulation : $u n'est pas nommée"
done
existe "$cible/debug/.fingerprint/vieux-3333333333333333"
existe "$cible/release/deps/libneuf-5555555555555555.rlib"
grep -qx "clippy --message-format=json --all-targets -- -D warnings" "$bac/appels" || ko "clippy rejoué autrement : $(cat "$bac/appels")"
grep -qx "test --message-format=json --no-run" "$bac/appels" || ko "test rejoué autrement : $(cat "$bac/appels")"

# 2. Balayage réel.
peupler
sortie="$(PATH="$bac:$PATH" "$SCRIPT" "$cible" "$projet" "test --no-run" 2>&1)" || ko "balayage : sortie non nulle : $sortie"
existe "$cible/debug/.fingerprint/neuf-1111111111111111"
existe "$cible/debug/deps/libneuf-1111111111111111.rlib"
existe "$cible/debug/deps/neuf-1111111111111111.d"
existe "$cible/debug/.fingerprint/neuf-2222222222222222"
existe "$cible/debug/build/neuf-2222222222222222/out/g.rs"
existe "$cible/debug/avash-ui"
absent "$cible/debug/.fingerprint/vieux-3333333333333333"
absent "$cible/debug/deps/libvieux-3333333333333333.rlib"
absent "$cible/debug/deps/vieux-3333333333333333.d"
absent "$cible/debug/build/vieux-3333333333333333"
absent "$cible/debug/.fingerprint/mon-paquet-4444444444444444"
absent "$cible/debug/deps/libmon_paquet-4444444444444444.rlib"
absent "$cible/release/.fingerprint/neuf-5555555555555555"
absent "$cible/release/deps/libneuf-5555555555555555.rlib"
grep -q "3 unités retirées" <<<"$sortie" || ko "le bilan ne compte pas 3 unités retirées : $sortie"

# 3. Énumération vide : refus, et rien n'est retiré.
peupler
if SILENCE=1 PATH="$bac:$PATH" "$SCRIPT" "$cible" "$projet" "test --no-run" >/dev/null 2>&1; then
  ko "une énumération vide devrait faire échouer le balayage"
fi
existe "$cible/debug/.fingerprint/vieux-3333333333333333"
existe "$cible/release/deps/libneuf-5555555555555555.rlib"

[ "$echec" -eq 0 ] || exit 1
echo "  ✓ balayer-target : ne retire que les unités que cargo n'énumère plus"
