#!/usr/bin/env bash
# Garde anti-étourderie : bloque les restes de mise au point dans le front avant
# qu'ils n'atterrissent dans un commit (harnais de test, debugger, auto-connexion
# vers un serveur de test). Rapide, sans build — appelée par check.sh et le hook.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

FILES="web/main.ts web/filters.ts web/index.html"
fail=0

forbid() { # motif  libellé
  local hits
  if hits=$(grep -REn "$1" $FILES 2>/dev/null); then
    echo "  ✗ $2 :"
    echo "$hits" | sed 's/^/      /'
    fail=1
  fi
}

# Marqueurs que le développement/tests laissent parfois traîner.
forbid "HARNESS TEMPORAIRE|À RETIRER|=== HARNESS" "harnais de test oublié"
forbid "\bdebugger\b" "instruction debugger"
forbid "127\.0\.0\.1:3389[0-9]" "auto-connexion vers un serveur de test"

if [ "$fail" -ne 0 ]; then
  echo "✗ garde front : reste(s) de mise au point détecté(s)." >&2
  exit 1
fi
echo "✓ garde front : rien à signaler"
