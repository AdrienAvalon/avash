#!/usr/bin/env bash
# Le harnais bout en bout pilote le binaire que désigne AVASH_E2E_APP, dans ses
# deux chemins (tauri-driver et serveur WebDriver embarqué), et le banc de
# mesures reste hors de la suite.
#
# Pourquoi : le banc (scripts/banc.mjs) mesure deux versions construites chacune
# dans son worktree. Si un des deux chemins gardait le binaire de l'arbre en
# dur, le banc comparerait la tête à elle-même sous Windows (chemin embarqué)
# ou sous Linux (tauri-driver), et conclurait « aucun écart » sur une vraie
# régression. Et si les spécifications du banc entraient dans la suite, le
# compteur de scénarios et la durée de la CI dériveraient.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
echecs=0
conf=e2e/wdio.conf.js

grep -qE 'process\.env\.AVASH_E2E_APP' "$conf" \
  || { echo "✗ $conf ne lit pas AVASH_E2E_APP"; echecs=$((echecs + 1)); }
# Le lancement embarqué et la capacité tauri-driver passent tous deux par APP.
grep -qE 'spawn\(APP,' "$conf" \
  || { echo "✗ le chemin embarqué ne lance pas APP"; echecs=$((echecs + 1)); }
grep -qE '"tauri:options": \{ application: APP \}' "$conf" \
  || { echo "✗ la capacité tauri-driver ne désigne pas APP"; echecs=$((echecs + 1)); }
if grep -nE 'target/release/avash-ui' "$conf" | grep -v 'join(import.meta.dirname' ; then
  echo "✗ un chemin de binaire en dur subsiste dans $conf"; echecs=$((echecs + 1))
fi

[ -f e2e/wdio.banc.conf.js ] || { echo "✗ e2e/wdio.banc.conf.js absent"; echecs=$((echecs + 1)); }
# La suite ne ramasse que e2e/specs : le banc vit à côté, jamais dedans.
grep -qE 'specs: \["\./specs/\*\*/\*\.spec\.js"\]' "$conf" \
  || { echo "✗ la suite ne se limite plus à ./specs"; echecs=$((echecs + 1)); }
if ls e2e/specs/ | grep -qi banc; then
  echo "✗ un fichier du banc est dans e2e/specs"; echecs=$((echecs + 1))
fi

if [ "$echecs" -gt 0 ]; then exit 1; fi
echo "✓ binaire désigné par AVASH_E2E_APP, banc hors de la suite"
