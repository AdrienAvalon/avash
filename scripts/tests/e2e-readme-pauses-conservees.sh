#!/usr/bin/env bash
# Contrôle reproductible : la section « Astuces WebKitGTK » d'e2e/README.md dit
# la vérité sur les `browser.pause` du code.
#
# Trouvé par l'audit du 8 septembre 2026 : la puce affirmait « Les browser.pause
# ont tous disparu », alors que les specs en gardent plusieurs — stabilisation de
# rendu avant capture visuelle (visuel.spec.js), boucle de retape du port série
# (serie.spec.js), nettoyage d'un tunnel encore ouvert (tunnels.spec.js). La
# règle réellement à tenir n'est pas « aucune pause » mais « jamais de pause
# fixe DEVANT une assertion négative » : là, la pause juge « rien n'est venu »
# avant que la chose ait eu le temps de venir, et le test passe même quand le
# comportement est cassé (cf. enregistrement.spec.js, corrigé le même jour).
# Un relecteur qui lit « il n'y en a plus » ne cherche pas ce motif fragile.
#
# Sources de vérité (le code), vérifiées ici avant de juger la doc :
#  - au moins un `browser.pause(` subsiste dans les specs → « tous disparu » est faux ;
#  - visuel/serie/tunnels en gardent effectivement (les pauses légitimes énumérées).
# Le contrôle rougit contre l'ancienne formulation absolue et exige que le README
# énumère les pauses conservées et pose la règle de l'assertion négative.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

readme="e2e/README.md"
specs="e2e/specs"

echecs=0

# --- Vérité du code : des browser.pause subsistent bel et bien ---------------
restants="$(grep -rl 'browser\.pause(' "$specs" 2>/dev/null || true)"
if [[ -z "$restants" ]]; then
  echo "  ✗ $specs : plus aucun browser.pause — la source de vérité a bougé," >&2
  echo "    « ont tous disparu » redeviendrait vrai ; revoir doc et ce contrôle" >&2
  echecs=1
fi
# Les trois specs aux pauses légitimes que le README doit justifier.
for spec in visuel serie tunnels; do
  if ! grep -qE 'browser\.pause\(' "$specs/$spec.spec.js"; then
    echo "  ✗ $specs/$spec.spec.js n'a plus de browser.pause : la pause conservée" >&2
    echo "    documentée dans le README a disparu, réaccorder les deux" >&2
    echecs=1
  fi
done

# --- Le README ne doit plus prétendre qu'elles ont toutes disparu -----------
if grep -qiE 'browser\.pause.*(ont tous disparu|tous disparu|ont disparu|plus aucun)' "$readme"; then
  echo "  ✗ $readme prétend encore que les browser.pause ont tous disparu" >&2
  echo "    → il en reste dans visuel/serie/tunnels ; énumérer celles conservées" >&2
  echecs=1
fi

# --- Le README doit poser la règle « jamais devant une assertion négative » -
if ! grep -qiE 'assertion négative' "$readme"; then
  echo "  ✗ $readme n'énonce pas la règle « jamais de pause fixe devant une assertion négative »" >&2
  echecs=1
fi

# --- Le README doit renvoyer aux états observables (requestAnimationFrame / sortiePty)
if ! grep -qE 'requestAnimationFrame' "$readme"; then
  echo "  ✗ $readme ne renvoie plus à requestAnimationFrame comme état observable" >&2
  echecs=1
fi

# --- Le README doit nommer les pauses conservées et leur raison -------------
for motif in 'capture' 'série' 'tunnel'; do
  if ! grep -qiE "$motif" "$readme"; then
    echo "  ✗ $readme n'évoque pas la pause conservée « $motif »" >&2
    echecs=1
  fi
done

if [[ "$echecs" -ne 0 ]]; then
  exit 1
fi

echo "  ✓ e2e/README.md : pauses conservées énumérées, règle de l'assertion négative posée"
