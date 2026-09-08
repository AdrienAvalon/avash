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
# Trouvé par l'audit du 8 septembre 2026 : la classe négative contenait un `.`
# qui laissait passer `window.confirm(`, `globalThis.prompt(`, `self.alert(` —
# justement les formes préfixées d'un objet global à proscrire. Le `.` est
# retiré et les préfixes globaux sont couverts explicitement ; aucun `.confirm(`
# ni `.prompt(` ni `.alert(` légitime n'existe dans web/ (si une méthode homonyme
# apparaît un jour, l'exclure nommément plutôt que par un `.` générique).
forbid "(^|[^a-zA-Z_\$])(window\.|globalThis\.|self\.)?(confirm|prompt)\(" "dialogue natif confirm()/prompt() (utiliser askConfirm/askText)"
# alert() est de la même famille : sous WebKitGTK/WRY il ne bloque pas et
# n'affiche pas nécessairement quoi que ce soit. Utiliser notify().
forbid "(^|[^a-zA-Z_\$])(window\.|globalThis\.|self\.)?alert\(" "dialogue natif alert() (utiliser notify)"

# ---------- Rust : restes de mise au point et de contrôle négatif ----------
#
# Trouvé le 8 septembre 2026 : un contrôle négatif (on neutralise un correctif
# pour vérifier que son test rougit, puis on le remet) a été laissé en place
# dans `run_avec_agent` — `break false, // CONTROLE NEGATIF` à la place de
# `break true`. Ni clippy, ni le format, ni les tests unitaires du module ne le
# voyaient ; seul un `check.sh` complet, relancé à la main, l'a attrapé par le
# test d'intégration. Le marqueur, lui, est trivial à repérer.
#
# Sources Rust du dépôt, hors paquets portés (`vendor/`, code amont) : le cœur,
# l'interface, le processus RDP et les serveurs de test.
# Seuls les répertoires présents sont passés à grep : sur un répertoire absent,
# grep -r sort en code 2 et un `if hits=$(...)` prendrait alors de VRAIS
# résultats pour une absence de résultat.
RUST_DIRS=()
for d in crates/avash/src crates/avash-ui/src rdp-sidecar/src test-rdp-server/src test-vnc-server/src; do
  [ -d "$d" ] && RUST_DIRS+=("$d")
done

forbid_rust() { # motif  libellé  [partout]
  # Par défaut les lignes de commentaire sont ignorées, comme pour le front (un
  # commentaire a le droit de dire « pas de dbg! ici »). Avec `partout`, RIEN
  # n'est ignoré : un marqueur de contrôle négatif vit précisément dans un
  # commentaire, souvent en fin de ligne de code, et n'a jamais sa place dans
  # une source commitée.
  local hits
  [ "${#RUST_DIRS[@]}" -eq 0 ] && return 0
  if [ "${3:-}" = "partout" ]; then
    hits=$(grep -rEn --include='*.rs' "$1" "${RUST_DIRS[@]}" 2>/dev/null) || hits=""
  else
    hits=$(grep -rEn --include='*.rs' "$1" "${RUST_DIRS[@]}" 2>/dev/null \
           | grep -vE '^[^:]+:[0-9]+:[[:space:]]*(//|/\*|\*)') || hits=""
  fi
  if [ -n "$hits" ]; then
    echo "  ✗ $2 :"
    echo "$hits" | sed 's/^/      /'
    fail=1
  fi
}

forbid_rust "CONTR[OÔ]LE N[EÉ]GATIF|CONTROLE_NEGATIF" "contrôle négatif laissé en place" partout
forbid_rust "\bdbg!\(" "macro dbg!() oubliée"
forbid_rust "\btodo!\(" "macro todo!() oubliée"

if [ "$fail" -ne 0 ]; then
  echo "✗ garde : reste(s) de mise au point détecté(s)." >&2
  exit 1
fi
echo "✓ garde front et Rust : rien à signaler"
