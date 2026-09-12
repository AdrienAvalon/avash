#!/usr/bin/env bash
# Contrôle reproductible : les permissions accordées à la webview
# (crates/avash-ui/capabilities/default.json) sont exactement celles que le
# front utilise.
#
# Trouvé par l'audit de sécurité du front du 12 septembre 2026 (FS-6, C-ipc-5) :
# `core:default` entraînait `core:webview:default` (dont
# `allow-internal-toggle-devtools`), `dialog:default` accordait les messages
# natifs jamais utilisés, `process:default` la sortie de l'application alors
# que seul `relaunch` sert. Une permission accordée est une surface offerte à
# tout script de la webview ; celle qui ne sert pas n'a rien à y faire.
#
# Les greffons se lisent dans les `import { … } from "@tauri-apps/plugin-…"`
# de web/*.ts (tests exclus) ; chaque fonction importée appelle une commande
# du greffon, dont la permission est donnée par la table ci-dessous.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# fonction importée (greffon/nom) → permission qu'elle exige.
declare -A PERMISSION=(
  [plugin-dialog/open]="dialog:allow-open"
  [plugin-dialog/save]="dialog:allow-save"
  [plugin-dialog/message]="dialog:allow-message"
  [plugin-dialog/ask]="dialog:allow-ask"
  [plugin-dialog/confirm]="dialog:allow-confirm"
  [plugin-process/relaunch]="process:allow-restart"
  [plugin-process/exit]="process:allow-exit"
  [plugin-clipboard-manager/readText]="clipboard-manager:allow-read-text"
  [plugin-clipboard-manager/writeText]="clipboard-manager:allow-write-text"
  [plugin-updater/check]="updater:default"
)

# Ensembles trop larges, refusés quel que soit l'usage.
PROSCRITES=(core:default core:webview:default core:menu:default core:tray:default dialog:default process:default clipboard-manager:default)

verifie() {
  local racine="$1" caps echec=0 attendues accordees p
  caps="$racine/crates/avash-ui/capabilities/default.json"
  # Permissions attendues d'après les imports du front.
  attendues="$(grep -rhoE --include='*.ts' --exclude='*.test.ts' --exclude-dir=node_modules --exclude-dir=dist \
      'import[[:space:]]*\{[^}]*\}[[:space:]]*from[[:space:]]*"@tauri-apps/plugin-[a-z-]+"' "$racine/web" |
    while IFS= read -r ligne; do
      greffon="$(sed -E 's/.*"@tauri-apps\/(plugin-[a-z-]+)".*/\1/' <<<"$ligne")"
      sed -E 's/^import[[:space:]]*\{([^}]*)\}.*/\1/' <<<"$ligne" | tr ',' '\n' |
        sed -E 's/^[[:space:]]*//; s/[[:space:]]+as[[:space:]].*//; s/[[:space:]]*$//' |
        while IFS= read -r nom; do
          [ -n "$nom" ] || continue
          cle="$greffon/$nom"
          if [ -n "${PERMISSION[$cle]:-}" ]; then
            echo "${PERMISSION[$cle]}"
          else
            echo "INCONNUE:$cle"
          fi
        done
    done | sort -u)"
  if grep -q '^INCONNUE:' <<<"$attendues"; then
    grep '^INCONNUE:' <<<"$attendues" | sed 's/^INCONNUE:/  ✗ import de greffon sans permission connue : /' >&2
    echo "    (compléter la table PERMISSION de ce script)" >&2
    echec=1
  fi
  accordees="$(grep -oE '"[a-z-]+:[a-z:-]+"' "$caps" | tr -d '"' | sort -u)"
  for p in $attendues; do
    case "$p" in INCONNUE:*) continue ;; esac
    if ! grep -qxF "$p" <<<"$accordees"; then
      echo "  ✗ le front utilise $p mais la capacité ne l'accorde pas" >&2
      echec=1
    fi
  done
  for p in $accordees; do
    case "$p" in core:*) continue ;; esac
    if ! grep -qxF "$p" <<<"$attendues"; then
      echo "  ✗ $p est accordée mais aucun import du front ne s'en sert" >&2
      echec=1
    fi
  done
  for p in "${PROSCRITES[@]}"; do
    if grep -qxF "$p" <<<"$accordees"; then
      echo "  ✗ $p est un ensemble trop large (détailler les permissions utilisées)" >&2
      echec=1
    fi
  done
  return "$echec"
}

echec=0
if ! verifie "$PWD"; then
  echec=1
fi

# Contrôles négatif et positif sur un dépôt factice.
bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT
mkdir -p "$bac/web" "$bac/crates/avash-ui/capabilities"
printf 'import { open as openDialog } from "@tauri-apps/plugin-dialog";\n' > "$bac/web/a.ts"
printf 'import { save } from "@tauri-apps/plugin-dialog";\n' > "$bac/web/a.test.ts"
cat > "$bac/crates/avash-ui/capabilities/default.json" <<'JSON'
{ "permissions": ["core:event:default", "core:webview:default", "dialog:allow-open", "dialog:allow-save", "process:default"] }
JSON
if verifie "$bac" 2>/dev/null; then
  echo "  ✗ contrôle négatif : des permissions trop larges ou inutilisées sont passées" >&2
  echec=1
fi
cat > "$bac/crates/avash-ui/capabilities/default.json" <<'JSON'
{ "permissions": ["core:event:default", "dialog:allow-open"] }
JSON
if ! verifie "$bac" 2>/dev/null; then
  echo "  ✗ contrôle positif : une capacité exactement ajustée au front a rougi" >&2
  echec=1
fi

if [ "$echec" -eq 0 ]; then
  echo "  ✓ capacités : exactement les permissions que le front utilise"
fi
exit "$echec"
