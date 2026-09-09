#!/usr/bin/env bash
# Contrôle reproductible : le hook de pré-commit ne rejoue pas ce que check.sh
# vient de valider sur le même arbre, et rejoue tout dès que l'arbre a changé.
#
# Trouvé par l'audit du 9 septembre 2026 : la porte est lente deux fois. Un
# `./check.sh` complet prend une quinzaine de minutes ; le commit qui suit
# relance dans le hook clippy, les tests, le sidecar et le front (huit minutes)
# sur un arbre strictement identique. check.sh écrit donc, quand il est vert,
# un témoin (.git/avash-temoin-check) qui porte l'empreinte de l'arbre validé
# (scripts/temoin-arbre.sh : HEAD, différences suivies, fichiers non suivis).
# Le hook accepte le commit sans rien rejouer si l'empreinte courante est celle
# du témoin ; tout autre cas, témoin absent ou arbre modifié depuis, passe par
# les vérifications comme avant.
#
# Ce contrôle rejoue le préambule du vrai hook (tout ce qui précède la première
# vérification) dans un dépôt jetable, avec le vrai script d'empreinte, et
# vérifie les trois cas : témoin à jour (aucune vérification atteinte, code 0),
# témoin périmé (vérifications atteintes), témoin absent (idem).
set -euo pipefail
RACINE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HOOK="$RACINE/scripts/hooks/pre-commit"
TEMOIN="$RACINE/scripts/temoin-arbre.sh"
[ -f "$HOOK" ] || { echo "  ✗ hook introuvable : $HOOK" >&2; exit 1; }
[ -f "$TEMOIN" ] || { echo "  ✗ script d'empreinte introuvable : $TEMOIN" >&2; exit 1; }
preambule="$(awk '/^echo "▸/{exit} {print}' "$HOOK")"

echecs=()
bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT

scenario() { # <dossier> <mode : a-jour | perime | absent>
  # Le dépôt jetable vit dans un sous-dossier : le hook rejoué, sa sortie et
  # le bilan restent à côté, hors de l'arbre, sinon ils compteraient comme des
  # fichiers non suivis et périmeraient le témoin qu'ils servent à éprouver.
  local dir="$1" mode="$2"
  mkdir -p "$dir/repo"
  (
    cd "$dir/repo"
    git init -q
    git config user.email test@avash
    git config user.name test
    git config commit.gpgsign false
    mkdir -p scripts
    cp "$TEMOIN" scripts/temoin-arbre.sh
    printf 'BON\n' > sentinelle.txt
    git add sentinelle.txt scripts/temoin-arbre.sh
    git commit -qm base
    printf 'SUITE\n' > sentinelle.txt
    git add sentinelle.txt
    case "$mode" in
      a-jour) bash scripts/temoin-arbre.sh > .git/avash-temoin-check ;;
      perime)
        bash scripts/temoin-arbre.sh > .git/avash-temoin-check
        printf 'ENCORE\n' > sentinelle.txt
        git add sentinelle.txt
        ;;
      absent) rm -f .git/avash-temoin-check ;;
    esac
    {
      printf '%s\n' "$preambule"
      printf 'echo ATTEINT\n'
    } > "$dir/hook.sh"
    set +e
    bash "$dir/hook.sh" > "$dir/sortie" 2>&1
    echo "code=$?"
    grep -q ATTEINT "$dir/sortie" && echo "atteint=oui" || echo "atteint=non"
  ) > "$dir/resultat" 2>/dev/null || true
}
champ() { sed -n "s/^$2=//p" "$1/resultat" | head -1; }

d="$bac/a-jour"; mkdir -p "$d"; scenario "$d" a-jour
[ "$(champ "$d" code)" = 0 ]      || echecs+=("témoin à jour : le hook refuse le commit (code $(champ "$d" code)) au lieu de l'accepter sur la foi de check.sh.")
[ "$(champ "$d" atteint)" = non ] || echecs+=("témoin à jour : le hook rejoue les vérifications que check.sh vient de passer.")

d="$bac/perime"; mkdir -p "$d"; scenario "$d" perime
[ "$(champ "$d" atteint)" = oui ] || echecs+=("témoin périmé : l'arbre a changé depuis check.sh et le hook ne rejoue pas les vérifications (faux vert).")

d="$bac/absent"; mkdir -p "$d"; scenario "$d" absent
[ "$(champ "$d" atteint)" = oui ] || echecs+=("témoin absent : le hook ne rejoue pas les vérifications.")

grep -q 'avash-temoin-check' "$RACINE/check.sh" || echecs+=("check.sh n'écrit pas le témoin .git/avash-temoin-check quand il est vert.")

if [ "${#echecs[@]}" -ne 0 ]; then
  printf '  ✗ %s\n' "${echecs[@]}" >&2
  exit 1
fi
echo "  ✓ hook : un arbre que check.sh vient de valider passe sans rejouer, tout autre arbre est revérifié"
