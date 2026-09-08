#!/usr/bin/env bash
# Contrôle reproductible de la porte des correctifs portés,
# rdp-sidecar/verifier-portes.sh.
#
# Trouvé par l'audit du 8 septembre 2026 : la sortie de `cargo test` était
# entièrement consommée par `grep | awk` pour compter les tests. Quand un
# paquet porté échouait (cargo sort 101) ou ne compilait plus, pipefail
# propageait le code dans l'affectation `n=$(…)`, `set -e` arrêtait le script :
# le code de sortie restait bon (non nul) mais RIEN n'était imprimé, ni le nom
# du test, ni l'erreur de compilation. En CI, l'étape « correctifs portés »
# rougissait après « <paquet> : N tests » puis silence, sans dire quel paquet
# ni quel test ; il fallait rejouer à la main dans chaque vendor/.
#
# Ce test remplace `cargo` par un faux qui simule un test porté en échec
# (sortie 101 avec un nom de test reconnaissable), rejoue le vrai
# verifier-portes.sh et exige DEUX choses : la porte reste rouge (code non nul)
# ET la sortie contient de quoi diagnostiquer (le nom du test échoué). Contre
# le script d'origine il échoue (sortie vide) ; après le correctif il passe.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

PORTE="rdp-sidecar/verifier-portes.sh"
MARQUEUR="le_cadrage_egfx_reprend_la_surface"

bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT

# Faux `cargo` : imite un `cargo test` dont un cas échoue. Il écrit sur stdout
# comme le fait cargo (récapitulatif « test result: FAILED »), nomme le test
# fautif, puis sort 101 comme cargo le fait sur échec de test.
cat > "$bac/cargo" <<EOF
#!/usr/bin/env bash
echo "running 4 tests"
echo "test $MARQUEUR ... FAILED"
echo ""
echo "failures:"
echo "    $MARQUEUR"
echo "test result: FAILED. 3 passed; 1 failed; 0 ignored"
exit 101
EOF
chmod +x "$bac/cargo"

sortie="$bac/sortie.txt"
code=0
PATH="$bac:$PATH" bash "$PORTE" > "$sortie" 2>&1 || code=$?

echecs=()

# Invariant 1 : un test porté en échec doit garder la porte rouge.
if [ "$code" -eq 0 ]; then
  echecs+=("la porte est verte alors qu'un test porté a échoué (code=$code)")
fi

# Invariant 2 : l'échec doit être visible dans le journal — c'est le défaut.
# Sans lui, l'étape CI rougit sans dire quel paquet ni quel test.
if ! grep -q "$MARQUEUR" "$sortie"; then
  echecs+=("l'échec d'un test porté ne fait apparaître aucune sortie de cargo (ni le test $MARQUEUR ni l'erreur) : la porte rougit sans rien à diagnostiquer")
fi

if [ "${#echecs[@]}" -ne 0 ]; then
  for e in "${echecs[@]}"; do
    printf '  ✗ %s\n' "$e" >&2
  done
  exit 1
fi

echo "  ✓ un test porté en échec fait rougir la porte ET imprime de quoi diagnostiquer"
