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
SERVEUR_RDP="$ROOT/test-rdp-server"
SERVEUR_VNC="$ROOT/test-vnc-server"
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

# Les serveurs de test sont eux aussi hors de l'espace de travail, et la chaîne
# ne faisait que les compiler : les 27 tests du côté serveur RDPDR (décodeurs
# écrits à la main, automate du scénario) ne tournaient nulle part. Même leçon
# que le processus RDP, même remède.
step "Serveurs de test (hors workspace)"
run "tests serveur RDP"  "$SERVEUR_RDP" cargo test
run "format serveur RDP" "$SERVEUR_RDP" cargo fmt --check
run "clippy serveur RDP" "$SERVEUR_RDP" cargo clippy --all-targets -- -D warnings
run "tests serveur VNC"  "$SERVEUR_VNC" cargo test
run "format serveur VNC" "$SERVEUR_VNC" cargo fmt --check
run "clippy serveur VNC" "$SERVEUR_VNC" cargo clippy --all-targets -- -D warnings
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
# Le manifeste de mise à jour (latest.json du workflow Release) doit proposer les
# cibles deb/rpm : sans elles, une installation par paquet se rabat sur l'AppImage
# et échoue à l'installer. Contrôle guardé par PyYAML (pas une dépendance du dépôt).
if python3 -c "import yaml" >/dev/null 2>&1; then
  run "manifeste maj (deb/rpm)" "$ROOT" ./scripts/tests/manifeste-maj.sh
  # L'étape « Couverture du front » de qualite.yml masquait l'échec de Vitest à
  # travers un tube (`| tee`) sans pipefail : une régression du front laissait
  # le job Qualité vert et le chiffre de couverture faux ou absent.
  run "qualité : échec vitest visible" "$ROOT" ./scripts/tests/qualite-couverture-front-echec.sh
  # Le job `fuzz` de securite.yml était sauté sur les PR et `check.sh` ne compile
  # pas le crate `fuzz` (hors espace de travail) : un renommage ou une signature
  # changée d'un parseur passait vert et ne cassait le job qu'après la fusion.
  run "sécurité : fuzz compile sur PR" "$ROOT" ./scripts/tests/securite-fuzz-compile-pr.sh
  # Le job `fuzz` jetait le corpus enrichi par la couverture (`fuzz/corpus`,
  # ignoré par git, hors du cache rust-cache) : l'exploration repartait des
  # seules graines à chaque poussée et chaque lundi, sans jamais réutiliser ce
  # que les runs précédents avaient découvert.
  run "sécurité : corpus de fuzz persistant" "$ROOT" ./scripts/tests/securite-fuzz-corpus-persistant.sh
  # Le manifeste Flathub doit accorder le son du bureau distant (--socket=pulseaudio,
  # webview WebKitGTK/GStreamer) et les consoles série (--device=all, /dev/ttyUSB*
  # et /dev/ttyACM*) : sans eux, ces deux fonctions sont muettes/vides dans le bac à
  # sable, alors qu'elles marchent en AppImage. Les droits sont à justifier (§8).
  run "flathub : son et série accordés" "$ROOT" ./scripts/tests/flathub-permissions-audio-serie.sh
fi
# release.sh s'annonce « à lancer SUR Windows » mais copiait le sidecar RDP sans
# extension : sous Windows le binaire est avash-rdp.exe et Tauri (externalBin)
# attend binaries/avash-rdp-<triple>.exe, si bien que `cp` échouait (set -e)
# avant `cargo tauri build`. Ce contrôle rejoue la vraie ligne de copie pour une
# cible Windows (suffixe .exe) et Linux (sans).
run "release : suffixe .exe du sidecar (windows)" "$ROOT" ./scripts/tests/release-sidecar-suffixe-windows.sh
# Sous Windows, le harnais e2e modifie le sshd du SYSTÈME (port 22) : ce chemin
# doit refuser de s'exécuter hors CI, sans quoi un `npm test` en terminal élevé
# écrase les clés d'admin de la machine et y laisse la clé de test.
run "garde sshd windows (e2e)" "$ROOT" node --test scripts/tests/sshd-windows-garde.mjs
# Le harnais e2e doit remettre à zéro le STOCKAGE WEB (langue, santé…) entre
# fichiers et le confiner au bac à sable : sinon les scénarios fuient l'un sur
# l'autre et, sur un poste où XDG_DATA_HOME est exporté, écrivent dans les
# données réelles de l'utilisateur.
run "isolation stockage web (e2e)" "$ROOT" node --test scripts/tests/e2e-isolation-stockage-web.mjs
# La relance du pilote WebDriver doit vivre dans le LANCEUR (onWorkerStart), pas
# dans beforeSession (processus de travail) : là-bas la poignée tauriDriver vaut
# undefined et relances repart de zéro, si bien qu'un pilote orphelin gardait le
# port 4444 et empoisonnait le run suivant.
run "relance pilote côté lanceur (e2e)" "$ROOT" node --test scripts/tests/e2e-relance-pilote-lanceur.mjs
# En CI, une référence visuelle absente doit faire ROUGIR l'étape : autoSaveBaseline
# doit être off en CI (sinon un tag ajouté/renommé sans PNG refabrique sa référence
# et passe vert sans rien comparer), rouvert par VISUEL_INIT pour amorcer.
run "régression visuelle : rougit sans référence en CI (e2e)" "$ROOT" node --test scripts/tests/e2e-visuel-baseline-ci.mjs
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
