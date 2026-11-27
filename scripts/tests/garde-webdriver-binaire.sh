#!/usr/bin/env bash
# Contrôle reproductible : scripts/verifier-sans-webdriver.sh rougit sur un
# binaire qui porte le serveur WebDriver embarqué, reste vert sur un binaire
# qui ne le porte pas, et son contrôle inverse exige TOUS les littéraux.
#
# Audit du 27 novembre 2026 (SE-2) : la publication vérifie désormais son
# binaire avec ce script. Une garde qui ne rougit jamais ne prouve rien : ce
# contrôle la rejoue sur de faux binaires (octets nuls autour, comme dans un
# `.rodata`). L'essai sur de vrais binaires, avec et sans `--features
# webdriver`, est décrit dans l'en-tête du script et rejoué par les jobs bout
# en bout de ci.yml (`--doit-contenir`).
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

garde=scripts/verifier-sans-webdriver.sh
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

faux() { # faux <fichier> <littéral>...
  local f="$1"; shift
  { head -c 4096 /dev/zero; for l in "$@"; do printf '%s\0' "$l"; done; head -c 4096 /dev/zero; } > "$f"
}

faux "$tmp/propre" "avash" "TAURI_WEBDRIVER" "AlertState"
faux "$tmp/webdriver" "TAURI_WEBDRIVER_PORT" "AlertStateManager lock poisoned"
faux "$tmp/moitie" "TAURI_WEBDRIVER_PORT"

echecs=0
attendu() { # attendu <code attendu> <libellé> <commande...>
  local code_attendu="$1" libelle="$2"; shift 2
  local code=0
  "$@" >/dev/null 2>&1 || code=$?
  if [ "$code" -ne "$code_attendu" ]; then
    echo "  ✗ $libelle : code $code, attendu $code_attendu" >&2
    echecs=1
  fi
}

attendu 0 "binaire sans greffon accepté" "$garde" "$tmp/propre"
attendu 1 "binaire avec greffon refusé" "$garde" "$tmp/webdriver"
attendu 1 "un seul littéral suffit à refuser" "$garde" "$tmp/moitie"
attendu 1 "un binaire propre parmi d'autres ne masque pas le fautif" "$garde" "$tmp/propre" "$tmp/webdriver"
attendu 1 "fichier absent refusé (rien vérifié n'est pas vert)" "$garde" "$tmp/absent"
attendu 2 "aucun binaire : usage" "$garde"
attendu 0 "contrôle inverse : binaire webdriver reconnu" "$garde" --doit-contenir "$tmp/webdriver"
attendu 1 "contrôle inverse : un littéral manquant rougit" "$garde" --doit-contenir "$tmp/moitie"
attendu 1 "contrôle inverse : binaire propre rougit" "$garde" --doit-contenir "$tmp/propre"

# Le greffon est épinglé en version exacte : une montée doit passer par une
# relecture des littéraux (contrôle inverse en CI).
if ! grep -Eq '^tauri-plugin-wdio-webdriver = \{ version = "=[0-9]+\.[0-9]+\.[0-9]+"' crates/avash-ui/Cargo.toml; then
  echo "  ✗ tauri-plugin-wdio-webdriver n'est plus épinglé en version exacte dans crates/avash-ui/Cargo.toml" >&2
  echecs=1
fi

[ "$echecs" -eq 0 ] || exit 1
echo "  ✓ la garde webdriver refuse un binaire qui porte le serveur embarqué, et son contrôle inverse exige tous les littéraux"
