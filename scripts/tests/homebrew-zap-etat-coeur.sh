#!/usr/bin/env bash
# Contrôle reproductible du `zap trash` du cask Homebrew : il doit effacer
# l'état du cœur, pas seulement les répertoires de la webview Tauri.
#
# Trouvé par l'audit du 7 septembre 2026 : `brew uninstall --zap avash` ne
# listait que les trois répertoires `dev.avash.app` (Application Support,
# Caches, WebKit) posés par Tauri. Or le cœur range son propre état sous
# `repertoire_configuration().join("avash")` (crates/avash : rdp.yaml,
# folders.yaml, snippets.yaml, tunnels.yaml, onglets.json, rdp_known_hosts,
# rdp_canal_graphique, enregistrements/), et sur macOS `dirs::config_dir()`
# rend `~/Library/Application Support`. Le zap laissait donc sur le disque les
# bureaux RDP, les tunnels, les snippets (parfois avec un jeton), les
# empreintes TOFU et les enregistrements de terminal — exactement ce qu'un zap
# promet d'effacer. Ce test ancre le nom du sous-dossier au code du cœur et
# exige que le cask vise `~/Library/Application Support/<dossier>` ; il rougit
# contre l'ancien état.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

CASK="packaging/homebrew/avash.rb"

# Nom du sous-dossier d'état, lu dans le code du cœur (source de vérité) :
# les crates font tous `repertoire_configuration()...join("avash")`.
if ! grep -q 'join("avash")' crates/avash/src/rdphost.rs; then
  echo "  ✗ crates/avash/src/rdphost.rs ne fait plus join(\"avash\") : adapter ce contrôle au nouveau nom d'état" >&2
  exit 1
fi
DOSSIER_ETAT="avash"

echecs=()

# La cible attendue : l'état du cœur sous Application Support (config_dir macOS).
CIBLE="~/Library/Application Support/${DOSSIER_ETAT}"
if ! grep -qF "\"${CIBLE}\"" "$CASK"; then
  echecs+=("le cask ne vise pas \"${CIBLE}\" dans zap trash : l'état du cœur (rdp.yaml, snippets.yaml, tunnels.yaml, empreintes TOFU, enregistrements) survit à brew uninstall --zap")
fi

# Garde-fou : les répertoires de la webview Tauri doivent rester listés.
for tauri in \
  "~/Library/Application Support/dev.avash.app" \
  "~/Library/Caches/dev.avash.app" \
  "~/Library/WebKit/dev.avash.app"; do
  if ! grep -qF "\"${tauri}\"" "$CASK"; then
    echecs+=("le cask ne vise plus \"${tauri}\" : la purge de la webview Tauri a régressé")
  fi
done

if ((${#echecs[@]})); then
  for e in "${echecs[@]}"; do
    echo "  ✗ $e" >&2
  done
  exit 1
fi

echo "  ✓ le cask efface l'état du cœur (~/Library/Application Support/${DOSSIER_ETAT}) et les répertoires Tauri au zap"
