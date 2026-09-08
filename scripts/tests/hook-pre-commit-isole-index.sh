#!/usr/bin/env bash
# Contrôle reproductible : le hook de pré-commit ne rend pas un verdict quand il
# ne peut pas représenter fidèlement le commit produit.
#
# Trouvé par l'audit du 8 septembre 2026 : le hook lançait fmt, clippy, tests,
# sidecar et front sur les fichiers de l'arbre de travail. Avec un ajout partiel
# (`git add -p`, ou un fichier modifié après `git add`), l'index diffère de
# l'arbre : le hook pouvait être vert alors que le commit produit ne compilait
# pas (le hunk non indexé portait la définition appelée par les hunks indexés,
# CI rouge sur main), et à l'inverse refuser un commit sain à cause de travail
# non indexé ailleurs. CLAUDE.md promet pourtant « le hook refuse un commit non
# formaté » : vrai seulement si l'index égale l'arbre.
#
# Le correctif NE remise PAS l'arbre pour isoler l'index : `git stash
# --keep-index` puis `git stash pop` corrompt l'arbre dans ce cas même (conflit
# rejoué, marqueurs <<<<<<< dans le fichier suivi — vérifié à l'audit, perte de
# travail). Le hook refuse le commit quand l'index diffère de l'arbre suivi, en
# expliquant pourquoi. Ce contrôle vérifie que 1) le hook s'arrête AVANT toute
# vérification lorsque index et arbre divergent, sans toucher ni l'arbre ni
# l'index ; 2) il laisse passer quand ils coïncident (pas de sur-blocage). Il
# rougit contre l'ancien hook (qui atteignait les vérifications sur un arbre
# divergent) et contre une réintroduction du stash lossy.
set -euo pipefail
RACINE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HOOK="$RACINE/scripts/hooks/pre-commit"

[ -f "$HOOK" ] || { echo "  ✗ hook introuvable : $HOOK" >&2; exit 1; }

# Préambule du hook = tout jusqu'à (hors) la première ligne « echo "▸ … » qui
# introduit une étape de vérification. Il porte la garde index/arbre.
preambule="$(awk '/^echo "▸/{exit} {print}' "$HOOK")"

echecs=()
bac_racine="$(mktemp -d)"
trap 'rm -rf "$bac_racine"' EXIT

# Construit un dépôt jouet dans $1 puis y joue le préambule du hook suivi d'un
# marqueur ATTEINT (preuve que les vérifications auraient tourné). $2 = "diverge"
# pour créer un ajout partiel (index=MAUVAIS, arbre=BON), "coincide" sinon
# (index=arbre=MAUVAIS). Écrit le code de sortie, la présence d'ATTEINT, l'état
# de l'arbre et de l'index dans $1/resultat.
scenario() {
  local dir="$1" mode="$2"
  (
    cd "$dir"
    git init -q
    git config user.email test@avash
    git config user.name test
    git config commit.gpgsign false
    printf 'BON\n' > sentinelle.txt
    git add sentinelle.txt
    git commit -qm base
    printf 'MAUVAIS\n' > sentinelle.txt
    git add sentinelle.txt          # index = MAUVAIS
    if [ "$mode" = diverge ]; then
      printf 'BON\n' > sentinelle.txt   # arbre = BON : ajout partiel
    fi
    # arbre = MAUVAIS sinon : index et arbre coïncident.
    {
      printf '%s\n' "$preambule"
      printf 'echo ATTEINT\n'
    } > hook.sh
    set +e
    bash hook.sh > sortie 2>&1
    echo "code=$?"
    grep -q ATTEINT sortie && echo "atteint=oui" || echo "atteint=non"
    echo "arbre=$(cat sentinelle.txt)"
    echo "index=$(git show :sentinelle.txt)"
  ) > "$dir/resultat" 2>/dev/null || true
}

champ() { sed -n "s/^$2=//p" "$1/resultat" | head -1; }

# --- 1. Index != arbre : le hook doit refuser AVANT les vérifications ---------
d="$bac_racine/diverge"; mkdir -p "$d"
scenario "$d" diverge
[ "$(champ "$d" code)" != 0 ]    || echecs+=("index != arbre : le hook rend un verdict (code 0) au lieu de refuser — il valide l'arbre, pas le commit (faux vert possible).")
[ "$(champ "$d" atteint)" = non ] || echecs+=("index != arbre : le hook atteint les vérifications au lieu de s'arrêter à la garde index/arbre.")
[ "$(champ "$d" arbre)" = BON ]  || echecs+=("index != arbre : le hook a modifié l'arbre de travail (attendu BON) — l'isolation ne doit rien perdre.")
[ "$(champ "$d" index)" = MAUVAIS ] || echecs+=("index != arbre : le hook a modifié l'index (attendu MAUVAIS).")

# --- 2. Index == arbre : le hook doit poursuivre (pas de sur-blocage) --------
d="$bac_racine/coincide"; mkdir -p "$d"
scenario "$d" coincide
[ "$(champ "$d" atteint)" = oui ] || echecs+=("index == arbre : le hook s'arrête à la garde alors que rien ne diverge — sur-blocage du cas courant (git add d'un fichier entier).")

# --- 3. Ancrage sur le hook réel ---------------------------------------------
grep -q 'git diff --quiet' "$HOOK" || echecs+=("scripts/hooks/pre-commit : pas de garde « git diff --quiet » (index vs arbre).")
# Le stash --keep-index corrompt l'arbre sur l'ajout partiel : il ne doit pas
# revenir comme « correctif ».
if grep -v '^[[:space:]]*#' "$HOOK" | grep -q -- '--keep-index'; then
  echecs+=("scripts/hooks/pre-commit : « git stash --keep-index » réintroduit — il corrompt l'arbre sur un ajout partiel (marqueurs de conflit). Refuser la divergence, ne pas remiser.")
fi

if [ "${#echecs[@]}" -ne 0 ]; then
  for e in "${echecs[@]}"; do echo "  ✗ $e" >&2; done
  exit 1
fi

echo "  ✓ le hook refuse quand index et arbre divergent (sans les toucher) et poursuit sinon"
