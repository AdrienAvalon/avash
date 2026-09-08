#!/usr/bin/env bash
# Contrôle reproductible : la liste des types de commit de CONTRIBUTING.md
# couvre tous les types Conventional Commits réellement employés dans
# l'historique, et CLAUDE.md ne tient pas une seconde liste concurrente (il
# renvoie à CONTRIBUTING.md).
#
# Trouvé par l'audit du 8 septembre 2026 : CONTRIBUTING.md énumérait feat, fix,
# perf, test, chore, docs — sans ci, build ni refactor, alors que l'historique
# comptait 33 commits `ci(...)`, 24 `build(...)` et 7 `refactor(...)`, les deux
# premiers étant les types les plus fréquents après fix/feat/test. CLAUDE.md, de
# son côté, tenait une liste DIFFÉRENTE (fix, feat, test, ci, docs, build,
# refactor) où manquaient chore et perf. Deux listes divergentes, aucune fidèle
# à l'usage : un contributeur externe ne savait pas s'il pouvait écrire `ci:` ou
# `build:` et récoltait une friction de revue.
#
# Source de vérité : les types canoniques Conventional Commits réellement
# présents dans `git log`. Quand l'historique n'est pas disponible (archive,
# clone superficiel trop court), on se rabat sur l'ensemble attendu figé, pour
# que le contrôle reste exécutable partout.
#
# Au prochain type canonique adopté (p. ex. `revert:`) qui s'installe dans
# l'historique, ce contrôle rougit tant que CONTRIBUTING.md ne le documente pas.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# Types Conventional Commits admis (liste fermée) : on ignore les préfixes
# informels d'avant l'adoption de la convention (rdp, outillage, design…), qui
# n'ont pas à figurer dans la doc.
canoniques=(feat fix perf test docs ci build refactor chore style revert)
est_canonique() {
  local t="$1"
  for c in "${canoniques[@]}"; do [[ "$c" == "$t" ]] && return 0; done
  return 1
}

# Ensemble attendu figé (repli si l'historique manque) : l'usage réel constaté.
attendus=(feat fix perf test docs ci build refactor chore)

# Seuil : un type doit apparaître au moins tant de fois pour être « employé »
# (un unique commit d'essai ne fige pas une convention).
seuil=3

echecs=0

# 1) Types réellement employés dans l'historique, filtrés aux canoniques.
declare -a employes=()
if git rev-parse --git-dir >/dev/null 2>&1 && [[ "$(git rev-list --count HEAD 2>/dev/null || echo 0)" -ge 50 ]]; then
  while read -r nb type; do
    [[ -z "$type" ]] && continue
    est_canonique "$type" || continue
    (( nb >= seuil )) && employes+=("$type")
  done < <(
    git log --format=%s \
      | grep -oE '^(feat|fix|perf|test|docs|ci|build|refactor|chore|style|revert)(\([^)]*\))?!?:' \
      | sed -E 's/(\([^)]*\))?!?:.*//' \
      | sort | uniq -c | sort -rn
  )
else
  echo "  ~ historique git indisponible : repli sur l'ensemble attendu figé" >&2
  employes=("${attendus[@]}")
fi

# 2) Types documentés dans CONTRIBUTING.md : les puces « - \`type:\` » ou
#    « - \`type(portée):\` » de la section « Format des commits ».
documentes="$(
  sed -n '/^## Format des commits/,/^## /p' CONTRIBUTING.md \
    | grep -oE '^- `[a-z]+' \
    | sed -E 's/^- `//' \
    | sort -u
)"

for t in "${employes[@]}"; do
  if ! grep -qx "$t" <<<"$documentes"; then
    echo "  ✗ CONTRIBUTING.md ne documente pas le type \`$t\` (employé dans l'historique)" >&2
    echecs=1
  fi
done

# 3) CLAUDE.md ne doit pas tenir une SECONDE liste de types : sa section
#    « Format des commits » renvoie à CONTRIBUTING.md sans énumérer les types.
#    L'ancienne formulation « (fix, feat, test, ci, docs, build, refactor) »
#    est la source de divergence à supprimer.
if [[ -f CLAUDE.md ]]; then
  section="$(sed -n '/^## Format des commits/,/^## /p' CLAUDE.md)"
  if ! grep -q 'CONTRIBUTING.md' <<<"$section"; then
    echo "  ✗ CLAUDE.md : la section « Format des commits » ne renvoie pas à CONTRIBUTING.md" >&2
    echecs=1
  fi
  # Une énumération de trois types ou plus séparés par des virgules entre
  # parenthèses est une seconde liste (au moins deux virgules).
  if grep -qE '\([a-z]+, [a-z]+, [a-z]+' <<<"$section"; then
    echo "  ✗ CLAUDE.md : la section « Format des commits » tient une seconde liste de types (divergence)" >&2
    echo "    → ne garder qu'un renvoi à CONTRIBUTING.md" >&2
    echecs=1
  fi
fi

if [[ "$echecs" -ne 0 ]]; then
  echo "  → aligner CONTRIBUTING.md sur l'usage réel : ${employes[*]}" >&2
  exit 1
fi

echo "  ✓ CONTRIBUTING.md couvre les types employés (${employes[*]}) ; CLAUDE.md renvoie sans doublon"
