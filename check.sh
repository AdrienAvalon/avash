#!/usr/bin/env bash
# Validation complète d'Avash : cœur, interface, front.
# Usage : ./check.sh [--quick]
#   --quick  saute le build release (plus rapide en boucle de dev)
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CORE="$ROOT/crates/avash"
UI="$ROOT/crates/avash-ui"
WEB="$ROOT/web"
SIDECAR="$ROOT/rdp-sidecar"
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
# Clippy ne compile qu'en debug : un bloc sous `cfg(debug_assertions)` peut
# laisser une variable inutilisée en release sans que rien ne le signale — c'est
# arrive. Ce passage-ci ne coute presque rien, le cache etant deja chaud.
run "clippy (release)"   "$ROOT" cargo clippy --workspace --release -- -D warnings

# Le sidecar RDP est HORS du workspace (conflit de versions pre-publication
# entre IronRDP et russh) : `--workspace` ne le voit pas. Ses tests — dont ceux
# du TOFU de certificat, le garde-fou qui empeche d'accepter n'importe quel
# serveur RDP — n'etaient donc executes NULLE PART, ni ici ni en CI, qui se
# contentait de le compiler. Ils passaient, mais personne ne l'aurait su s'ils
# avaient cesse de passer.
step "Processus RDP (hors workspace)"
run "compilation"        "$SIDECAR" cargo check --all-targets
run "tests"              "$SIDECAR" cargo test
# Les correctifs portés (cf. rdp-sidecar/vendor/README.md) ont leurs propres
# tests. Le script compte ceux qui s'exécutent : ces commandes ont longtemps
# réussi sans rien lancer, les manifestes vendorisés portant « test = false ».
run "correctifs portés"  "$SIDECAR" ./verifier-portes.sh
run "format"             "$SIDECAR" cargo fmt --check
run "clippy"             "$SIDECAR" cargo clippy --all-targets -- -D warnings
# Vulnerabilites connues des dependances. cargo-audit s'installe avec
#   cargo install cargo-audit --locked
if cargo audit --version >/dev/null 2>&1; then
  # On echoue sur les vulnerabilites, pas sur les avertissements
  # « unmaintained » : ils viennent tous de la pile GTK que Tauri embarque,
  # hors de notre controle.
  run "audit securite"     "$ROOT" cargo audit --deny unsound --ignore RUSTSEC-2023-0071 --ignore RUSTSEC-2024-0429
  # Le sidecar RDP est hors du workspace (conflit de versions pre-publication
  # entre IronRDP et russh) mais il est COMPILE ET LIVRE : son Cargo.lock doit
  # etre audite lui aussi, sans quoi ses dependances ne sont jamais regardees.
  run "audit sidecar RDP"  "$ROOT" cargo audit --deny unsound --ignore RUSTSEC-2023-0071 --file rdp-sidecar/Cargo.lock
else
  printf '  \033[33m~\033[0m %s\n' "audit securite (cargo-audit absent)"
fi

# cargo-audit ne voit que les vulnerabilites declarees. cargo-deny ferme trois
# autres portes, qu'aucun outil ne surveillait : une licence inattendue arrivant
# par une dependance transitive, une dependance en joker qui rend la
# construction imprevisible, et une source hors du registre officiel.
#   cargo install cargo-deny --locked
if cargo deny --version >/dev/null 2>&1; then
  run "licences et sources"  "$ROOT"    cargo deny check advisories licenses bans sources
  run "licences (RDP)"       "$SIDECAR" cargo deny check advisories licenses bans sources
else
  printf '  \033[33m~\033[0m %s\n' "licences et sources (cargo-deny absent)"
fi

# Conformite RDP contre de VRAIS serveurs xrdp, en conteneur. Hors du passage
# par defaut : demarrer le parc coute une minute et exige podman. Mais c'est le
# seul controle qui aurait vu les trois defauts de la 0.3.3 — image cisaillee,
# clavier en QWERTY, connexion suspendue. Aucun test en memoire ne les voyait.
#   ./scripts/parc-rdp.sh up tous && CONFORMITE_RDP=1 ./check.sh
if [ -n "${CONFORMITE_RDP:-}" ]; then
  step "Conformite RDP (serveurs xrdp reels)"
  run "conformite"         "$ROOT" ./scripts/conformite.sh "${PARC:-xfce}"
fi

step "Front (avash-web)"
run "garde"              "$ROOT" ./scripts/guard.sh
run "lint"               "$WEB" npx eslint .
# Le CSS vit dans index.html : stylelint le lit à travers postcss-html.
run "lint css"           "$WEB" npx stylelint index.html
# knip : fichiers jamais importés, exports jamais lus, dépendances jamais
# utilisées. Il a vu deux modules décrochés par le découpage du front que ni
# tsc ni ESLint ne pouvaient voir — chacun compilait, personne ne le chargeait.
run "code mort"          "$WEB" npx knip
run "typage"             "$WEB" npx tsc --noEmit
# Les dépendances du front vivent dans la webview, celles de la suite bout en
# bout sur la machine de développement : les deux arbres sont audités, au
# niveau « haute » et au-delà — un avis modéré sur un outil de test ne doit
# pas bloquer une correction, mais doit se voir.
run "audit npm (front)"  "$WEB" "$ROOT/scripts/npm-audit.sh" high
# La suite bout en bout dépend de WebdriverIO 9, dont quelques dépendances
# transitives (extract-zip, deepmerge-ts, serialize-javascript) portent des
# avis « haute » sans correctif en amont : ce code ne tourne que sur la
# machine de test, jamais chez l'utilisateur. On ne bloque que sur « critique ».
run "audit npm (e2e)"    "$ROOT/e2e" "$ROOT/scripts/npm-audit.sh" critical tolerer-registre
run "tests"              "$WEB" npx vitest run
run "build"              "$WEB" npx vite build

if [ "$QUICK" != "--quick" ]; then
  step "Build release"
  # `externalBin` de tauri.conf.json exige le binaire du sidecar AVANT toute
  # compilation d'avash-ui. Il n'est pas versionne : sur un clone neuf, cette
  # etape echouait ici alors qu'elle passait en CI, qui le construit, elle.
  run "processus RDP"    "$SIDECAR" cargo build --release
  cible="$ROOT/crates/avash-ui/binaries/avash-rdp-$(rustc -vV | sed -n 's/^host: //p')"
  mkdir -p "$(dirname "$cible")"
  cp "$SIDECAR/target/release/avash-rdp" "$cible" 2>/dev/null || true
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
