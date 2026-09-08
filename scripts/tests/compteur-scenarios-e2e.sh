#!/usr/bin/env bash
# Contrôle reproductible : le nombre de scénarios bout en bout annoncé dans
# CONTRIBUTING.md et dans e2e/README.md égale le décompte réel des `it(` de
# e2e/specs, et le nombre de fichiers annoncé égale le nombre de fichiers de
# specs.
#
# Trouvé par l'audit du 8 septembre 2026 : CONTRIBUTING.md annonçait « 69
# scénarios » (chiffre figé à la 0.9.0) alors que la suite en comptait 72 ;
# e2e/README.md, lui, disait 71 (il avait suivi deux ajouts de la 0.9.2 mais pas
# le dernier). Un contributeur qui compare deux nombres différents ne sait
# lequel croire. Cause structurelle : CLAUDE.md énumérait les fichiers porteurs
# de compteurs sans citer ces deux-là, si bien qu'ils dérivaient en silence.
#
# Ne sont gardés ici que les deux fichiers de cette voie (CONTRIBUTING.md,
# e2e/README.md). Les autres compteurs (README, docs, site) vivent hors de ce
# périmètre et sont tenus à jour à part.
#
# La source de vérité est le décompte des `it(` : au prochain scénario ajouté,
# ce contrôle rougit tant que les deux fichiers ne citent pas le nouveau total.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# Décompte réel : une déclaration `it(` par ligne, en tête de ligne.
reel_scenarios="$(grep -hcE '^\s*it\(' e2e/specs/*.spec.js | paste -sd+ - | { read -r somme; echo $((somme)); })"
reel_fichiers="$(ls e2e/specs/*.spec.js | wc -l | tr -d ' ')"

echecs=0

# CONTRIBUTING.md : « … le détail des NN scénarios. »
contrib="$(grep -oE 'détail des [0-9]+ scénarios' CONTRIBUTING.md | grep -oE '[0-9]+' || true)"
if [[ "$contrib" != "$reel_scenarios" ]]; then
  echo "  ✗ CONTRIBUTING.md annonce ${contrib:-?} scénarios, la suite en compte $reel_scenarios" >&2
  echecs=1
fi

# e2e/README.md : « ## Couverture (NN scénarios, MM fichiers) »
readme_sc="$(grep -oE 'Couverture \([0-9]+ scénarios' e2e/README.md | grep -oE '[0-9]+' || true)"
readme_fi="$(grep -oE '[0-9]+ fichiers\)' e2e/README.md | grep -oE '[0-9]+' || true)"
if [[ "$readme_sc" != "$reel_scenarios" ]]; then
  echo "  ✗ e2e/README.md annonce ${readme_sc:-?} scénarios, la suite en compte $reel_scenarios" >&2
  echecs=1
fi
if [[ "$readme_fi" != "$reel_fichiers" ]]; then
  echo "  ✗ e2e/README.md annonce ${readme_fi:-?} fichiers, e2e/specs en compte $reel_fichiers" >&2
  echecs=1
fi

if [[ "$echecs" -ne 0 ]]; then
  echo "  → mettre CONTRIBUTING.md et e2e/README.md à $reel_scenarios scénarios ($reel_fichiers fichiers)" >&2
  exit 1
fi

echo "  ✓ CONTRIBUTING.md et e2e/README.md : $reel_scenarios scénarios, $reel_fichiers fichiers"
