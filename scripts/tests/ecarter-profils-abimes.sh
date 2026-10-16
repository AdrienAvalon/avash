#!/usr/bin/env bash
# Contrôle reproductible de scripts/ecarter-profils-abimes.sh : un profil de
# couverture abîmé est écarté, les bons restent, et la fusion de ce qui reste
# réussit.
#
# Régression vue au workflow qualité le 16 octobre 2026 : un profil écrit au
# moment où le pilote WebDriver tuait l'application faisait refuser toute la
# fusion (« no profile can be merged »). Ce test compile un petit programme
# instrumenté, en tire un profil sain, un tronqué et un à compteur 2^64 - 1
# (ce que la CI a vu), et exige : les deux abîmés mis de côté, le sain gardé,
# la fusion du dossier possible, et un dossier sans profil sain en erreur.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

SCRIPT="$PWD/scripts/ecarter-profils-abimes.sh"
[ -x "$SCRIPT" ] || { echo "  ✗ $SCRIPT absent ou non exécutable" >&2; exit 1; }

profdata=$(ls "$(rustc --print sysroot)"/lib/rustlib/*/bin/llvm-profdata 2>/dev/null | head -1 || true)
if [ -z "$profdata" ]; then
  # Le composant llvm-tools n'est posé que là où l'on mesure la couverture.
  echo "  - llvm-tools absent : contrôle sauté (rustup component add llvm-tools)"
  exit 0
fi

bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT
cat > "$bac/a.rs" <<'RS'
fn f(x: u32) -> u32 { if x > 3 { x * 2 } else { x + 1 } }
fn main() { let mut s = 0; for i in 0..10 { s += f(i); } println!("{s}"); }
RS
rustc -C instrument-coverage -O "$bac/a.rs" -o "$bac/a" 2>/dev/null
mkdir -p "$bac/target"
LLVM_PROFILE_FILE="$bac/target/sain.profraw" "$bac/a" >/dev/null
taille=$(stat -c%s "$bac/target/sain.profraw")
cp "$bac/target/sain.profraw" "$bac/target/tronque.profraw"
truncate -s $((taille - 40)) "$bac/target/tronque.profraw"
cp "$bac/target/sain.profraw" "$bac/target/compteur-plein.profraw"
printf '\xff\xff\xff\xff\xff\xff\xff\xff' | dd of="$bac/target/compteur-plein.profraw" bs=1 seek=$((taille - 64)) conv=notrunc 2>/dev/null

# Témoin : sans le tri, la fusion de l'ensemble échoue comme en CI.
if "$profdata" merge -sparse "$bac"/target/*.profraw -o /dev/null >/dev/null 2>&1; then
  echo "  ✗ le témoin fusionne malgré deux profils abîmés : le test ne prouve rien" >&2; exit 1
fi

sortie=$("$SCRIPT" "$bac/target")
echo "$sortie" | grep -q "1 profil(s) gardé(s), 2 écarté(s)" || { echo "  ✗ bilan inattendu : $sortie" >&2; exit 1; }
[ -e "$bac/target/sain.profraw" ] || { echo "  ✗ le profil sain a été écarté" >&2; exit 1; }
[ -e "$bac/target/profils-ecartes/tronque.profraw" ] || { echo "  ✗ le profil tronqué est resté" >&2; exit 1; }
[ -e "$bac/target/profils-ecartes/compteur-plein.profraw" ] || { echo "  ✗ le profil à compteur plein est resté" >&2; exit 1; }
"$profdata" merge -sparse "$bac"/target/*.profraw -o /dev/null >/dev/null 2>&1 || { echo "  ✗ la fusion échoue encore après le tri" >&2; exit 1; }

# Plus aucun profil sain : erreur, pas un rapport vide présenté comme mesuré.
mkdir -p "$bac/vide"
cp "$bac/target/profils-ecartes/tronque.profraw" "$bac/vide/"
if "$SCRIPT" "$bac/vide" >/dev/null 2>&1; then
  echo "  ✗ un dossier sans profil sain passe" >&2; exit 1
fi
echo "  ✓ profils abîmés écartés, fusion du reste possible"
