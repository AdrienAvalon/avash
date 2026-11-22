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

# Révision du 22 novembre 2026 : WebdriverIO 10 exige de lui-même les versions
# saines de deepmerge-ts, serialize-javascript et @puppeteer/browsers. La
# condition de retrait que ce contrôle surveillait s'est réalisée (il a rougi,
# comme prévu) ; les trois overrides sont retirés et le contrôle garde
# maintenant l'état d'après : ils ne reviennent pas sans raison écrite, les
# versions vulnérables ne rentrent pas dans le verrou, et la doc décrit les
# overrides qui restent, ni plus ni moins.

# 1. Les overrides retirés ne reviennent pas en silence.
for dep in "deepmerge-ts" "serialize-javascript" "@puppeteer/browsers"; do
  if node -e 'const o=require("./e2e/package.json").overrides||{}; process.exit(process.argv[1] in o ? 0 : 1)' "$dep"; then
    echo "  ✗ $pkg redéclare l'override $dep, retiré le 22 novembre 2026 : dire pourquoi dans $readme et revoir ce contrôle" >&2
    echecs=1
  fi
done

# 2. Le verrou ne contient ni extract-zip ni les lignes vulnérables.
if grep -q "extract-zip" "$lock"; then
  echo "  ✗ $lock contient de nouveau extract-zip (GHSA-jmr9-qjv8-65gv)" >&2
  echecs=1
fi
if ! node -e '
  const p = require("./e2e/package-lock.json").packages;
  const majeure = (v) => Number(String(v).split(".")[0]);
  const mineure = (v) => Number(String(v).split(".")[1]);
  let mauvais = [];
  for (const [chemin, v] of Object.entries(p)) {
    const nom = chemin.split("node_modules/").pop();
    if (nom === "deepmerge-ts" && majeure(v.version) < 8) mauvais.push(`${chemin}@${v.version}`);
    if (nom === "@puppeteer/browsers" && majeure(v.version) < 3) mauvais.push(`${chemin}@${v.version}`);
    if (nom === "serialize-javascript"
        && (majeure(v.version) < 7 || (majeure(v.version) === 7 && mineure(v.version) < 1))) {
      mauvais.push(`${chemin}@${v.version}`);
    }
  }
  if (mauvais.length) { console.error("  ✗ versions vulnérables dans le verrou e2e : " + mauvais.join(", ")); process.exit(1); }
'; then
  echecs=1
fi

# 3. Chaque override qui reste est expliqué dans la doc, et la doc n'en
#    présente pas d'autre comme actif.
restants="$(node -e 'console.log(Object.keys(require("./e2e/package.json").overrides||{}).join("\n"))')"
while read -r dep; do
  [[ -n "$dep" ]] || continue
  if ! grep -qF "\`$dep\`" "$readme"; then
    echo "  ✗ $readme n'explique pas l'override $dep déclaré dans $pkg" >&2
    echecs=1
  fi
done <<<"$restants"
if ! grep -q "Conditions de retrait" "$readme"; then
  echo "  ✗ $readme ne dit plus à quelle condition retirer les overrides restants" >&2
  echecs=1
fi

# 4. Formulations périmées (constats d'origine du 8 septembre 2026).
if grep -qiE 'avis restants viennent .*extract-zip' "$readme"; then
  echo "  ✗ $readme affirme encore « les avis restants viennent tous d'extract-zip », qui a quitté le verrou" >&2
  echecs=1
fi
if grep -qiE 'transitives \(extract-zip' "$check"; then
  echo "  ✗ $check nomme encore extract-zip comme dépendance transitive à avis (commentaire audit npm e2e)" >&2
  echecs=1
fi

if [[ "$echecs" -ne 0 ]]; then
  exit 1
fi
echo "  ✓ e2e/README.md : overrides restants expliqués, overrides retirés absents, verrou sans version vulnérable ni extract-zip"
