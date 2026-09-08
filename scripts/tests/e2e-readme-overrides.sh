#!/usr/bin/env bash
# Contrôle reproductible : la section « Dépendances : les overrides de
# package.json » d'e2e/README.md décrit les overrides réels, et check.sh ne
# nomme plus extract-zip comme dépendance transitive à avis « haute ».
#
# Trouvé par l'audit du 8 septembre 2026 : e2e/package.json force TROIS
# overrides (deepmerge-ts, serialize-javascript, @puppeteer/browsers), mais la
# section du README n'en expliquait que deux et affirmait « les avis restants
# viennent tous d'extract-zip ». Or le troisième override existe justement pour
# évincer extract-zip (GHSA-jmr9-qjv8-65gv) : @puppeteer/browsers 3 l'a remplacé
# par modern-tar, et le verrou e2e ne contient plus extract-zip. Un mainteneur
# qui suit le README retirait ou ignorait l'override @puppeteer/browsers sans
# savoir qu'il ramenait extract-zip dans l'arbre. Le commentaire de check.sh
# (audit npm e2e) recopiait la même liste périmée.
#
# Sources de vérité (le code), vérifiées ici avant de juger la doc :
#  - e2e/package.json déclare les trois overrides ;
#  - extract-zip n'est plus dans e2e/package-lock.json ;
#  - @wdio/utils exige encore @puppeteer/browsers ^2.x (l'override force la 3,
#    condition de retrait : @wdio/utils passe à ≥ 3).
# Le contrôle rougit contre l'ancienne formulation du README et de check.sh.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

pkg="e2e/package.json"
lock="e2e/package-lock.json"
readme="e2e/README.md"
check="check.sh"

echecs=0

# --- Vérité du code : les trois overrides sont bien déclarés ----------------
for dep in "deepmerge-ts" "serialize-javascript" "@puppeteer/browsers"; do
  if ! grep -qF "\"$dep\"" "$pkg"; then
    echo "  ✗ $pkg ne déclare plus l'override $dep : la source de vérité a bougé, revoir la doc et ce contrôle" >&2
    echecs=1
  fi
done

# --- Vérité du verrou : extract-zip n'est plus dans l'arbre e2e -------------
if grep -q "extract-zip" "$lock"; then
  echo "  ✗ $lock contient encore extract-zip : la doc peut en reparler, revoir ce contrôle" >&2
  echecs=1
fi

# --- Vérité du verrou : @wdio/utils exige encore @puppeteer/browsers 2.x ----
# (justifie que l'override force la 3 ; condition de retrait = passage à ≥ 3)
if ! grep -qE '"@puppeteer/browsers": "\^2\.' "$lock"; then
  echo "  ✗ $lock : @wdio/utils n'exige plus @puppeteer/browsers ^2.x — la condition de retrait de l'override a changé, revoir la doc" >&2
  echecs=1
fi

# --- Le README doit documenter le troisième override -----------------------
if ! grep -qF "@puppeteer/browsers" "$readme"; then
  echo "  ✗ $readme n'explique pas l'override @puppeteer/browsers (le 3e, qui évince extract-zip)" >&2
  echo "    → décrire : v3 remplace extract-zip par modern-tar ; retrait quand @wdio/utils exige ≥ 3" >&2
  echecs=1
fi

# --- Le README ne doit plus dire que les avis restants viennent d'extract-zip
if grep -qiE 'avis restants viennent .*extract-zip' "$readme"; then
  echo "  ✗ $readme affirme encore « les avis restants viennent tous d'extract-zip », qui a quitté le verrou" >&2
  echecs=1
fi

# --- check.sh ne doit plus lister extract-zip comme dépendance transitive ---
# Le commentaire de l'étape « audit npm (e2e) » énumérait extract-zip parmi les
# dépendances transitives portant des avis « haute ».
if grep -qiE 'transitives \(extract-zip' "$check"; then
  echo "  ✗ $check nomme encore extract-zip comme dépendance transitive à avis (commentaire audit npm e2e)" >&2
  echecs=1
fi

if [[ "$echecs" -ne 0 ]]; then
  exit 1
fi

echo "  ✓ e2e/README.md et check.sh : overrides décrits conformément à package.json et au verrou (extract-zip évincé)"
