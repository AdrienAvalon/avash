#!/usr/bin/env bash
# Contrôle reproductible : dans .github/workflows/release.yml, le job `publier`
# doit attendre un job `securite` qui rejoue gitleaks et la campagne de fuzz.
#
# Trouvé le 8 septembre 2026 : le tag déclenchait Release indépendamment du
# workflow Sécurité (qui suit la poussée sur main). La 0.10.0 a été publiée
# pendant que le fuzzing et gitleaks rougissaient sur ce même commit, et il a
# fallu une 0.10.1. Ce contrôle exige que la publication dépende d'une chaîne de
# sécurité jouée dans le même workflow, et que cette chaîne contienne bien les
# deux contrôles qui ont mordu.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

wf=".github/workflows/release.yml"
echecs=0

# Le job securite existe.
if ! grep -qE '^  securite:$' "$wf"; then
  echo "  ✗ $wf : job « securite » introuvable" >&2; echecs=1
fi

# Le bloc du job securite (jusqu'au prochain job de premier niveau) contient
# gitleaks et fuzz.sh.
bloc="$(awk '/^  securite:$/{f=1; next} f && /^  [a-z_-]+:$/{exit} f{print}' "$wf")"
if ! grep -q 'gitleaks/gitleaks-action@' <<<"$bloc"; then
  echo "  ✗ $wf : le job securite ne joue pas gitleaks" >&2; echecs=1
fi
if ! grep -q 'fuzz/fuzz.sh' <<<"$bloc"; then
  echo "  ✗ $wf : le job securite ne joue pas fuzz/fuzz.sh" >&2; echecs=1
fi

# publier dépend de build ET de securite.
needs="$(awk '/^  publier:$/{f=1; next} f && /^    needs:/{print; exit}' "$wf")"
for job in build securite; do
  if ! grep -qE "\b$job\b" <<<"$needs"; then
    echo "  ✗ $wf : publier n'attend pas « $job » (needs : ${needs:-absent})" >&2; echecs=1
  fi
done

if [ "$echecs" -ne 0 ]; then
  echo "✗ release-publier-attend-securite : la publication n'attend pas la sécurité." >&2
  exit 1
fi
echo "✓ release-publier-attend-securite : publier attend build et securite (gitleaks + fuzz)"
