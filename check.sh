#!/usr/bin/env bash
# Validation complète d'Avash : cœur, interface, front.
# Usage : ./check.sh [--quick]
#   --quick  saute le build release (plus rapide en boucle de dev)
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CORE="$ROOT/crates/avash"
UI="$ROOT/crates/avash-ui"
WEB="$ROOT/web"
QUICK=${1:-}
FAILED=()

step() { printf '\n\033[1;36m▸ %s\033[0m\n' "$1"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$1"; }
bad()  { printf '  \033[31m✗\033[0m %s\n' "$1"; FAILED+=("$1"); }

run() { # run <libellé> <répertoire> <commande...>
  local label="$1" dir="$2"; shift 2
  if [ ! -d "$dir" ]; then bad "$label (répertoire absent : $dir)"; return; fi
  if (cd "$dir" && "$@" >/tmp/avash-check.$$ 2>&1); then
    ok "$label"
  else
    bad "$label"
    tail -25 /tmp/avash-check.$$ | sed 's/^/      /'
  fi
  rm -f /tmp/avash-check.$$
}

# Le workspace valide les deux crates Rust d'un seul appel : dependances
# communes compilees une fois, target partage.
step "Rust (workspace : avash + avash-ui)"
run "compilation"        "$ROOT" cargo check --workspace --all-targets
run "tests"              "$ROOT" cargo test --workspace --all-targets
run "format"             "$ROOT" cargo fmt --all --check
run "clippy"             "$ROOT" cargo clippy --workspace --all-targets -- -D warnings

step "Front (avash-web)"
run "typage"             "$WEB" npx tsc --noEmit
run "tests"              "$WEB" npx vitest run
run "build"              "$WEB" npx vite build

if [ "$QUICK" != "--quick" ]; then
  step "Build release"
  run "binaire Tauri"    "$ROOT" cargo build --release -p avash-ui
fi

printf '\n'
if [ ${#FAILED[@]} -eq 0 ]; then
  printf '\033[1;32m✓ Tout est vert.\033[0m\n'
  exit 0
fi
printf '\033[1;31m✗ %d étape(s) en échec :\033[0m\n' "${#FAILED[@]}"
printf '  - %s\n' "${FAILED[@]}"
exit 1
