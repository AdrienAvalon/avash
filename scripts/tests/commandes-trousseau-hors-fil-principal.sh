#!/usr/bin/env bash
# Contrôle reproductible : aucune commande Tauri synchrone ne touche au
# trousseau, au disque ou à un processus.
#
# Trouvé par l'audit du 12 septembre 2026 (C-SIL-7, C-perf-2, C-perf-10) : une
# commande `#[tauri::command]` sans `async` s'exécute sur le fil principal, celui
# qui peint la fenêtre et sert l'IPC (tauri-macros, `ExecutionContext::Blocking`).
# Pendant que KWallet attendait son mot de passe au clic sur un bureau enregistré,
# Avash ne repeignait plus rien et ne répondait plus. Désormais une commande
# synchrone doit figurer dans la liste ci-dessous, avec sa raison, et son corps
# ne doit rien contenir de bloquant ; toutes les autres sont `async fn` (le
# trousseau passant alors par `bloquant`/`spawn_blocking`) ou portent
# `#[tauri::command(async)]`.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# Commandes synchrones admises : mémoire seulement, sauf `keyboard_locks`.
AUTORISEES=(
  keyboard_locks          # GetKeyState lit l'état du fil appelant (C-unsafe-5)
  canal_de_mise_a_jour    # une variable d'environnement et l'estampille du binaire
  open_sessions           # le magasin des sessions, en mémoire
  tunnel_status           # le magasin des tunnels, en mémoire
  sftp_annuler            # lève un drapeau en mémoire
  rdp_diagnostic          # le journal du sidecar, en mémoire
  rdp_close               # retire l'enfant du magasin et lui envoie un signal
  pty_ack                 # accusé du terminal, en mémoire, très fréquent
  diagnostic_noter_rendu  # retient « webgl » ou « dom », en mémoire
  snippet_vars            # analyse d'une chaîne (non exposée)
  enregistrement_en_cours # le chemin retenu en mémoire (non exposée)
)

# Ce qu'un corps de commande synchrone ne doit pas contenir.
INTERDITS='secrets::|open::that|Command::new|std::fs::|puttygen|from_alias|depuis_alias|parse_ssh_config|configuration_resolue|load_hosts|load_defs|repertoire|folders::'

# commandes_synchrones <racine> : « fichier:ligne:nom » de chaque commande
# synchrone (attribut `#[tauri::command]` nu suivi d'un `pub fn`).
commandes_synchrones() {
  local racine="$1"
  find "$racine/crates/avash-ui/src" -name '*.rs' ! -name 'tests*.rs' -print0 |
    xargs -0 awk '
      FNR == 1 { attente = 0 }
      /^[[:space:]]*#\[tauri::command\][[:space:]]*$/ { attente = 1; next }
      attente && /^[[:space:]]*(#\[|\/\/)/ { next }
      attente {
        if (match($0, /^[[:space:]]*pub fn [a-z_0-9]+/)) {
          nom = substr($0, RSTART, RLENGTH); sub(/.*pub fn /, "", nom)
          print FILENAME ":" FNR ":" nom
        }
        attente = 0
      }'
}

# corps <fichier> <ligne> : le corps de la fonction qui commence à <ligne>,
# jusqu'à la première accolade fermante en début de ligne.
corps() {
  awk -v debut="$2" 'FNR >= debut { print; if (FNR > debut && /^}/) exit }' "$1"
}

verifie() {
  local racine="$1" echec=0 fichier ligne nom autorisee a
  while IFS=: read -r fichier ligne nom; do
    [ -n "$nom" ] || continue
    autorisee=0
    for a in "${AUTORISEES[@]}"; do
      if [ "$a" = "$nom" ]; then autorisee=1; fi
    done
    if [ "$autorisee" -eq 0 ]; then
      echo "  ✗ $nom (${fichier#"$racine"/}:$ligne) est une commande synchrone : elle s'exécute sur le fil principal ; la rendre async ou #[tauri::command(async)]" >&2
      echec=1
    elif corps "$fichier" "$ligne" | grep -qE "$INTERDITS"; then
      echo "  ✗ $nom (${fichier#"$racine"/}:$ligne) est admise synchrone mais touche au trousseau, au disque ou à un processus" >&2
      echec=1
    fi
  done < <(commandes_synchrones "$racine")
  return "$echec"
}

echec=0

# --- 1. Le dépôt lui-même.
if ! verifie "$PWD"; then
  echec=1
fi

# --- 2. Contrôles négatifs et positif sur un dépôt factice.
bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT
mkdir -p "$bac/crates/avash-ui/src"
cat > "$bac/crates/avash-ui/src/a.rs" <<'RUST'
#[tauri::command]
#[must_use]
pub fn mot_de_passe_connu(compte: String) -> bool {
    avash::secrets::load(&compte).is_some()
}
RUST
if verifie "$bac" 2>/dev/null; then
  echo "  ✗ contrôle négatif : une commande synchrone qui lit le trousseau est passée" >&2
  echec=1
fi
cat > "$bac/crates/avash-ui/src/a.rs" <<'RUST'
#[tauri::command]
pub fn rdp_diagnostic(id: u64) -> String {
    std::fs::read_to_string("/etc/passwd").unwrap_or_default()
}
RUST
if verifie "$bac" 2>/dev/null; then
  echo "  ✗ contrôle négatif : une commande admise synchrone qui lit le disque est passée" >&2
  echec=1
fi
cat > "$bac/crates/avash-ui/src/a.rs" <<'RUST'
#[tauri::command]
pub async fn mot_de_passe_connu(compte: String) -> bool {
    bloquant(move || Ok(avash::secrets::load(&compte))).await.is_ok()
}

#[tauri::command(async)]
pub fn liste() -> Result<Vec<String>, String> {
    std::fs::read_dir("/").map(|_| Vec::new()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pty_ack(id: u64, seq: u64) {}
RUST
if ! verifie "$bac" 2>/dev/null; then
  echo "  ✗ contrôle positif : des commandes hors du fil principal ont rougi" >&2
  echec=1
fi

if [ "$echec" -eq 0 ]; then
  echo "  ✓ commandes : trousseau, disque et processus hors du fil principal"
fi
exit "$echec"
