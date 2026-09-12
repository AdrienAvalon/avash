#!/usr/bin/env bash
# Contrôle reproductible : la politique de sécurité du contenu de la webview
# ferme ce qui ne sert pas.
#
# Trouvé par l'audit de sécurité du front du 12 septembre 2026 (FS-5, C-ipc-4) :
# `base-uri` et `form-action` ne retombent pas sur `default-src`, et n'étaient
# pas posées ; `connect-src` admettait `ws://localhost:*`, que le front n'utilise
# jamais (il ne vise que `ws://127.0.0.1:<port>` du sidecar), ce qui ouvrait la
# page à tout service WebSocket local. `style-src 'unsafe-inline'` reste :
# xterm.js crée des éléments <style> à l'exécution.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# verifie <tauri.conf.json> : rend 1 si la CSP n'est pas fermée.
verifie() {
  local conf="$1" csp echec=0 d
  csp="$(grep -oE '"csp"[[:space:]]*:[[:space:]]*"[^"]*"' "$conf" | sed -E 's/^"csp"[[:space:]]*:[[:space:]]*"//; s/"$//')"
  if [ -z "$csp" ]; then
    echo "  ✗ aucune CSP lue dans $conf" >&2
    return 1
  fi
  for d in "default-src 'self'" "script-src 'self'" "object-src 'none'" "base-uri 'none'" "form-action 'none'" "frame-src 'none'"; do
    if ! grep -qF "$d" <<<"$csp"; then
      echo "  ✗ la CSP doit porter « $d »" >&2
      echec=1
    fi
  done
  if grep -qE 'localhost' <<<"$csp"; then
    echo "  ✗ la CSP ne doit pas admettre localhost (le front ne vise que 127.0.0.1)" >&2
    echec=1
  fi
  if grep -qE "unsafe-eval|'unsafe-hashes'|wasm-unsafe-eval" <<<"$csp"; then
    echo "  ✗ la CSP ne doit admettre aucune évaluation de code" >&2
    echec=1
  fi
  if grep -oE "script-src[^;]*" <<<"$csp" | grep -qE "unsafe-inline|data:|\*"; then
    echo "  ✗ script-src doit rester « 'self' »" >&2
    echec=1
  fi
  if grep -oE "connect-src[^;]*" <<<"$csp" | grep -qE "https?:|wss:|ws://[^1]"; then
    echo "  ✗ connect-src ne doit viser que la page et ws://127.0.0.1" >&2
    echec=1
  fi
  return "$echec"
}

echec=0
if ! verifie crates/avash-ui/tauri.conf.json; then
  echec=1
fi

# Contrôle négatif : l'ancienne CSP doit rougir.
bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT
cat > "$bac/tauri.conf.json" <<'JSON'
{ "app": { "security": {
  "csp": "default-src 'self'; object-src 'none'; connect-src 'self' ws://127.0.0.1:* ws://localhost:*; style-src 'self' 'unsafe-inline'; font-src 'self' data:"
} } }
JSON
if verifie "$bac/tauri.conf.json" 2>/dev/null; then
  echo "  ✗ contrôle négatif : la CSP d'avant l'audit est passée" >&2
  echec=1
fi

if [ "$echec" -eq 0 ]; then
  echo "  ✓ CSP : directives fermées, ni localhost ni évaluation"
fi
exit "$echec"
