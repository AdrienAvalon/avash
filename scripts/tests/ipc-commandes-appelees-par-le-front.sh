#!/usr/bin/env bash
# Contrôle reproductible : toute commande exposée à la webview par
# `generate_handler!` (crates/avash-ui/src/lib.rs) doit être appelée par le
# front (web/*.ts). Une commande enregistrée est une surface offerte à tout ce
# qui s'exécute dans la webview ; celle qui ne sert à rien n'a rien à y faire.
#
# Trouvé par l'audit du 9 septembre 2026 : `enregistrement_en_cours` restait
# enregistrée alors que le front pilote son état d'enregistrement par les
# valeurs de retour de `enregistrement_demarrer` / `enregistrement_arreter`.
# La politique était déjà écrite en commentaire dans lib.rs (trois commandes
# retirées pour cette raison), mais rien ne la faisait respecter : la quatrième
# a été oubliée. Ce script relit la liste et compare, à chaque check.sh.
#
# Les appels du front sont des chaînes littérales (`invoke("sftp_list", …)`),
# jamais composées : chercher le nom entre guillemets suffit et ne peut pas
# blanchir une commande par hasard. Les fichiers de test du front ne comptent
# pas comme appelants.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# verifie <racine> : liste les commandes de <racine>/crates/avash-ui/src/lib.rs
# que rien n'appelle dans <racine>/web ; rend 1 s'il y en a.
verifie() {
  local racine="$1" lib commandes nom manque=0
  lib="$racine/crates/avash-ui/src/lib.rs"
  commandes="$(sed -n '/generate_handler!\[/,/\]/p' "$lib" \
    | grep -oE '[a-z_]+::[a-z_0-9]+' | grep -v '^tauri::' | sort -u)"
  [ -n "$commandes" ] || { echo "  ✗ aucune commande lue dans $lib" >&2; return 1; }
  while read -r commande; do
    nom="${commande##*::}"
    if ! grep -rqF --include='*.ts' --exclude='*.test.ts' \
         --exclude-dir=node_modules --exclude-dir=dist \
         "\"$nom\"" "$racine/web"; then
      echo "  ✗ $commande est exposée à la webview mais aucun appel du front ne l'utilise" >&2
      manque=1
    fi
  done <<<"$commandes"
  return "$manque"
}

echec=0

# --- 1. Le dépôt lui-même.
if ! verifie "$PWD"; then
  echo "  ✗ une commande enregistrée sans appelant : la retirer de generate_handler! (elle reste publique, donc testée)" >&2
  echec=1
fi

# --- 2. Contrôle négatif : un dépôt factice où une commande n'est pas appelée
# doit rougir, sinon ce script ne prouve rien.
bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT
mkdir -p "$bac/crates/avash-ui/src" "$bac/web"
cat > "$bac/crates/avash-ui/src/lib.rs" <<'RUST'
        .invoke_handler(tauri::generate_handler![
            commands::list_hosts,
            commands::commande_orpheline
        ])
RUST
printf 'invoke("list_hosts");\n' > "$bac/web/main.ts"
printf 'invoke("commande_orpheline");\n' > "$bac/web/main.test.ts"
if verifie "$bac" 2>/dev/null; then
  echo "  ✗ contrôle négatif : une commande appelée seulement par un test du front est passée pour utilisée" >&2
  echec=1
fi
# Et le même dépôt factice, une fois l'appel réel posé, doit verdir.
printf 'invoke("commande_orpheline");\n' >> "$bac/web/main.ts"
if ! verifie "$bac" 2>/dev/null; then
  echo "  ✗ contrôle positif : un dépôt factice dont toutes les commandes sont appelées a rougi" >&2
  echec=1
fi

if [ "$echec" -eq 0 ]; then
  echo "  ✓ IPC : chaque commande exposée à la webview a un appelant dans le front"
fi
exit "$echec"
