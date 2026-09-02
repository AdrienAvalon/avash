#!/usr/bin/env bash
# Garde anti-étourderie : bloque les restes de mise au point dans le front avant
# qu'ils n'atterrissent dans un commit (harnais de test, debugger, auto-connexion
# vers un serveur de test). Rapide, sans build — appelée par check.sh et le hook.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

FILES="web/*.ts web/index.html"
fail=0

forbid() { # motif  libellé
  local hits
  # Les lignes de commentaire sont ignorées : la documentation a le droit de
  # nommer ce qu'elle proscrit (« ne pas utiliser alert() »), et l'interdire
  # rendrait la garde impossible à expliquer dans le code lui-même.
  if hits=$(grep -REn "$1" $FILES 2>/dev/null \
            | grep -vE "^[^:]+:[0-9]+: *(//|/\*|\*|<!--)"); then
    echo "  ✗ $2 :"
    echo "$hits" | sed 's/^/      /'
    fail=1
  fi
}

# Marqueurs que le développement/tests laissent parfois traîner.
forbid "HARNESS TEMPORAIRE|À RETIRER|=== HARNESS" "harnais de test oublié"
forbid "\bdebugger\b" "instruction debugger"
forbid "127\.0\.0\.1:3389[0-9]" "auto-connexion vers un serveur de test"

# Dialogues natifs bloquants : INOPÉRANTS sous WebKitGTK/WRY (confirm renvoie une
# Promise toujours vraie, prompt renvoie null). Utiliser askConfirm()/askText().
forbid "(^|[^.a-zA-Z])(confirm|prompt)\(" "dialogue natif confirm()/prompt() (utiliser askConfirm/askText)"
# alert() est de la même famille : sous WebKitGTK/WRY il ne bloque pas et
# n'affiche pas nécessairement quoi que ce soit. Utiliser notify().
forbid "(^|[^.a-zA-Z])alert\(" "dialogue natif alert() (utiliser notify)"

if [ "$fail" -ne 0 ]; then
  echo "✗ garde front : reste(s) de mise au point détecté(s)." >&2
  exit 1
fi
echo "✓ garde front : rien à signaler"
