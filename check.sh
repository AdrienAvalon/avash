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

# Prerequis a la compilation d'avash-ui : son build.rs (tauri_build) exige deux
# artefacts NON versionnes — web/dist (frontendDist) et binaries/avash-rdp-<triplet>
# (externalBin, le sidecar). check.sh les fabriquait APRES la section Rust (front
# ligne ~247, sidecar dans « Build release » saute par --quick), si bien que sur
# un clone neuf `cargo check --workspace` echouait des la premiere etape (tauri
# panique sur « resource path … doesn't exist » puis « frontendDist … doesn't
# exist ») et que --quick n'etait JAMAIS vert sans les commandes manuelles de
# CONTRIBUTING (trouve par l'audit du 8 septembre 2026). ci.yml, lui, construit
# deja front puis sidecar avant sa section Rust : on aligne check.sh sur cet ordre.
step "Prerequis (front dist + sidecar)"
run "front (dist)"       "$WEB" npx vite build
run "processus RDP"      "$SIDECAR" cargo build --release
cible="$ROOT/crates/avash-ui/binaries/avash-rdp-$(rustc -vV | sed -n 's/^host: //p')"
mkdir -p "$(dirname "$cible")"
# Sans `|| true` : un echec de copie du sidecar doit rougir ici, sinon la section
# Rust echoue plus loin avec un message opaque de tauri_build.
run "depot du sidecar"   "$SIDECAR" cp "target/release/avash-rdp" "$cible"

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
  # hors de notre controle. Les avis acceptes sont dans .cargo/audit.toml, lu
  # automatiquement par cargo-audit (trouve par l'audit du 8 septembre 2026 :
  # les --ignore ici divergeaient d'un audit.toml racine que rien ne lisait).
  run "audit securite"     "$ROOT" cargo audit --deny unsound
  # Le sidecar RDP est hors du workspace (conflit de versions pre-publication
  # entre IronRDP et russh) mais il est COMPILE ET LIVRE : son Cargo.lock doit
  # etre audite lui aussi, sans quoi ses dependances ne sont jamais regardees.
  # Lance depuis $ROOT, il lit le meme .cargo/audit.toml.
  run "audit sidecar RDP"  "$ROOT" cargo audit --deny unsound --file rdp-sidecar/Cargo.lock
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
# La garde front doit proscrire les dialogues natifs sous TOUTES leurs formes,
# y compris préfixées (window.confirm(, globalThis.prompt(, self.alert() : un
# `.` dans la classe négative les laissait passer (audit du 8 septembre 2026).
run "garde : dialogues natifs préfixés proscrits" "$ROOT" ./scripts/tests/guard-dialogues-natifs-globaux.sh
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
  # Le `zap trash` du cask ne listait que les répertoires `dev.avash.app` de la
  # webview Tauri, pas l'état du cœur sous `~/Library/Application Support/avash`
  # (config_dir macOS) : brew uninstall --zap laissait bureaux RDP, tunnels,
  # snippets et empreintes TOFU sur le disque. Le contrôle exige la ligne d'état.
  run "homebrew : zap efface l'état du cœur" "$ROOT" ./scripts/tests/homebrew-zap-etat-coeur.sh
  # rust-cache ne cache par défaut que `./target` : le sidecar RDP et les
  # serveurs de test, hors espace de travail, recompilaient IronRDP/rustls/tokio
  # à chaque passage CI, faute d'être listés dans `workspaces` (là où GitLab a un
  # cache-sidecar dédié). Le contrôle exige la déclaration par job qui les bâtit.
  run "CI : rust-cache couvre le sidecar" "$ROOT" ./scripts/tests/ci-cache-sidecar.sh
  # Dans un bloc `files: |`, une ligne commençant par `#` fait partie de la
  # valeur, pas un commentaire : action-gh-release recevait cinq lignes de
  # commentaire comme motifs de fichiers. Le contrôle exige qu'aucune liste de
  # fichiers (`files`, `subject-path`) ne porte de ligne `#` dans le bloc.
  run "release : liste de fichiers sans commentaire" "$ROOT" ./scripts/tests/release-files-sans-commentaire.sh
  # La section Rust de check.sh compile avash-ui, dont le build.rs exige web/dist
  # et le binaire du sidecar (tous deux non versionnes) : le contrôle exige que ces
  # deux prerequis soient fabriques AVANT le premier `cargo … --workspace` (comme
  # ci.yml) et que le depot du sidecar ne soit pas silence par `|| true`.
  run "check.sh : prerequis avant la section Rust" "$ROOT" ./scripts/tests/check-prerequis-avant-rust.sh
fi
# Le hook de pré-commit vérifie l'arbre de travail : avec un ajout partiel
# (git add -p, ou un fichier modifié après git add) l'index diverge de l'arbre,
# et le verdict ne reflète pas le commit produit. Le contrôle exige que le hook
# refuse la divergence au lieu de rendre un faux vert (ou un faux rouge).
run "hook : refuse un index qui diverge de l'arbre" "$ROOT" ./scripts/tests/hook-pre-commit-isole-index.sh
# rdp-sidecar/verifier-portes.sh comptait les tests portés dans le tube même
# (`n=$(cargo test | grep | awk)`) : un test porté en échec arrêtait le script
# sans imprimer une seule ligne de cargo, la porte rougissait sans dire quel
# paquet ni quel test. Le contrôle rejoue la porte avec un faux cargo en échec.
run "porté : un échec de test reste visible" "$ROOT" ./scripts/tests/porte-sidecar-echec-visible.sh
# release.sh s'annonce « à lancer SUR Windows » mais copiait le sidecar RDP sans
# extension : sous Windows le binaire est avash-rdp.exe et Tauri (externalBin)
# attend binaries/avash-rdp-<triple>.exe, si bien que `cp` échouait (set -e)
# avant `cargo tauri build`. Ce contrôle rejoue la vraie ligne de copie pour une
# cible Windows (suffixe .exe) et Linux (sans).
run "release : suffixe .exe du sidecar (windows)" "$ROOT" ./scripts/tests/release-sidecar-suffixe-windows.sh
# captures-readme.sh lisait le mot de passe RDP par `CAPTURES_RDP_MDP="$(secret-tool
# lookup …)"` : secret-tool sort non nul quand le trousseau est vide, si bien que
# sous `set -e` le script mourait AVANT la garde qui disait quoi enregistrer (arrêt
# muet, code 1). Le contrôle rejoue les deux vraies lignes avec un secret-tool en
# échec et exige que le message diagnostique s'affiche.
run "captures : message trousseau atteignable sous set -e" "$ROOT" ./scripts/tests/captures-message-trousseau-atteignable.sh
# tracer-rdp.sh passait le mot de passe du bureau visé en argument du sidecar
# (`-p "$MDP"`) : contrairement à conformite.sh (compte de test), un vrai mot de
# passe restait lisible dans /proc/<pid>/cmdline pendant toute la capture. Le
# contrôle rejoue le vrai bloc d'invocation avec un faux sidecar et exige que le
# mot de passe arrive par stdin, absent de l'argv.
run "tracer-rdp : mot de passe au sidecar par stdin, pas en argument" "$ROOT" ./scripts/tests/tracer-rdp-mdp-par-stdin.sh
# conformite.sh et parc-rdp.sh aiguillaient le parc par `[ "$quoi" = "xfce" ] ||
# [ "$quoi" = "tous" ] && …` : un argument inconnu (« xcfe », « tout ») ne jouait
# aucun contrôle et rendait pourtant un verdict vert, code 0 (« tout est vert »,
# « parc prêt »). Le contrôle rejoue les deux scripts avec un argument inconnu et
# exige un code non nul et un usage ; il vérifie aussi la garde de non-vacuité de
# conformite.sh (aucun verdict vert si aucun contrôle n'a été joué).
run "conformite : argument de parc inconnu refusé" "$ROOT" ./scripts/tests/conformite-argument-inconnu.sh
# parc-rdp.sh raccorder : `moi=$(grep … containers/<id64> … | cut …)` sous
# `set -euo pipefail` mourait dès que grep ne trouvait rien (code 1) ou ne
# pouvait lire mountinfo (code 2) : l'affectation nue prenait ce code, errexit
# sortait AVANT le repli `[ -n "$moi" ] || moi="$(hostname)"` — `up` s'arrêtait
# muet code 1 sur un runtime/montage atypique. Le contrôle rejoue les deux
# vraies lignes avec un mountinfo sans identifiant puis illisible et exige que
# le repli hostname reste atteignable.
run "parc-rdp : repli hostname atteignable sous set -e" "$ROOT" ./scripts/tests/parc-rdp-repli-hostname-atteignable.sh
# parc-rdp.sh down détachait par `$(hostname)`, que raccorder documente comme
# n'étant PAS l'identifiant du conteneur sur l'exécuteur GitLab : le disconnect
# échouait (avalé), l'endpoint du job restait attaché, `network rm avash-parc`
# échouait à son tour (« active endpoints », avalé) et le réseau fuyait pipeline
# après pipeline. Le contrôle rejoue la vraie ligne de disconnect avec un
# mountinfo portant un identifiant connu et un hostname distinct, et exige que
# down détache par l'identifiant (via identifiant_conteneur, partagé avec raccorder).
run "parc-rdp : down détache par l'identifiant du conteneur" "$ROOT" ./scripts/tests/parc-rdp-down-detache-par-identifiant.sh
# Le [workspace.package] se disait « un seul endroit à modifier », mais les deux
# crates membres codaient leur version/edition/license en dur au lieu de
# `*.workspace = true` : bumper le workspace ne changeait rien aux binaires. Le
# contrôle exige que les membres héritent et que les emplacements réellement
# distincts (workspace, sidecar, tauri.conf.json, web/package.json) s'accordent.
run "release : version héritée du workspace et cohérente" "$ROOT" ./scripts/tests/version-heritee-et-coherente.sh
# CONTRIBUTING.md, à la racine, renvoyait à `[tests-parc](../tests-parc/README.md)` :
# le `../` sort du dépôt et fait une 404 sur GitHub, alors que le bon chemin sans
# `../` est déjà utilisé plus bas. Le contrôle résout chaque lien relatif des docs
# de la racine depuis leur dossier et exige que la cible existe.
run "docs : liens relatifs de la racine valides" "$ROOT" ./scripts/tests/docs-liens-relatifs.sh
# CONTRIBUTING.md annonçait « quatre portes » (dont le hook de pré-commit) pour
# toute crate hors workspace, mais le hook — barrière rapide — ne joue pas les
# tests de test-rdp-server/test-vnc-server : la règle était démentie par le
# dépôt. Le contrôle exige que les trois portes obligatoires jouent ces serveurs
# et que la règle ne sur-promette pas le hook.
run "docs : règle des portes cohérente avec le hook" "$ROOT" ./scripts/tests/portes-crates-hors-workspace.sh
# CONTRIBUTING.md annonçait « 69 scénarios » (figé à la 0.9.0) quand la suite en
# comptait 72, et e2e/README.md 71 : deux nombres divergents pour un même
# compteur. Le contrôle prend la somme des `it(` de e2e/specs pour vérité et
# exige que ces deux fichiers la citent (les autres compteurs vivent ailleurs).
run "docs : compteur de scénarios e2e à jour" "$ROOT" ./scripts/tests/compteur-scenarios-e2e.sh
# e2e/README.md attribuait le serveur RDP de test à `onPrepare` (« démarre aussi
# un serveur RDP local 33899 »), alors qu'onPrepare ne monte que le sshd et que
# chaque spec RDP/VNC lance son propre serveur ; et « En CI (E2E_NO_RDP=1)… »
# décrivait un état révolu (seul macOS pose E2E_NO_RDP ; Linux/Windows jouent la
# suite complète). Le contrôle prend le code pour vérité et rougit contre les
# deux anciennes formulations.
run "docs : e2e/README, serveurs locaux et E2E_NO_RDP exacts" "$ROOT" ./scripts/tests/e2e-readme-serveurs-locaux.sh
# e2e/README.md affirmait « Les browser.pause ont tous disparu », alors que dix
# subsistent (stabilisation avant capture visuelle, boucle de retape série,
# nettoyage de tunnel, et des pauses fragiles devant assertion négative traitées
# par ailleurs) : un relecteur qui s'y fie reproduisait le motif `pause` puis
# `expect(...).not...` en le croyant conforme. Le contrôle prend le code pour
# vérité (des browser.pause restent) et exige que le README les énumère et pose
# la règle « jamais de pause fixe devant une assertion négative ».
run "docs : e2e/README, pauses conservées et règle assertion négative" "$ROOT" ./scripts/tests/e2e-readme-pauses-conservees.sh
# e2e/package.json force TROIS overrides (deepmerge-ts, serialize-javascript,
# @puppeteer/browsers), mais e2e/README.md n'en décrivait que deux et affirmait
# « les avis restants viennent tous d'extract-zip » : or le 3e override évince
# justement extract-zip (@puppeteer/browsers 3 le remplace par modern-tar) et le
# verrou e2e ne le contient plus. Le contrôle prend package.json et le verrou
# pour vérité, exige que le README documente le 3e override et rougit contre la
# phrase extract-zip du README comme contre le commentaire périmé de check.sh.
run "docs : e2e/README, overrides et extract-zip évincé" "$ROOT" ./scripts/tests/e2e-readme-overrides.sh
# CONTRIBUTING.md listait feat/fix/perf/test/chore/docs — sans ci, build ni
# refactor, pourtant ci (33) et build (24) sont les types les plus fréquents
# après fix/feat/test dans l'historique ; CLAUDE.md tenait une SECONDE liste
# encore différente. Le contrôle prend les types canoniques de `git log` pour
# vérité et exige que CONTRIBUTING.md les documente, CLAUDE.md se bornant à un
# renvoi (une seule liste, plus de divergence possible).
run "docs : types de commit alignés sur l'historique" "$ROOT" ./scripts/tests/commit-types-alignes.sh
# README.md portait une section `## Documentation` (7 liens : CHANGELOG,
# feuille-de-route, qualite, architecture, SECURITY, tests-parc, RELEASE)
# ajoutée seule côté français (af64386) ; README.en.md passait de Contributing
# à License sans jamais l'avoir, si bien qu'un lecteur anglophone ne trouvait ni
# RELEASE.md ni tests-parc/README.md. Le contrôle prend la section de README.md
# pour vérité et exige que README.en.md porte la sienne avec les mêmes cibles.
run "docs : README.en.md porte la section Documentation" "$ROOT" ./scripts/tests/readme-en-section-documentation.sh
# L'étape 1 de RELEASE.md n'énumérait que les fichiers qui déclarent la version
# et oubliait deux choses qu'un tag emporte : l'entrée `<release>` du metainfo
# (embarqué par l'AUR/Flathub depuis l'archive du tag → logithèque figée à la
# version précédente sans elle) et les deux Cargo.lock (builds --frozen du
# PKGBUILD et de Flathub sinon en échec). Elle disait aussi « les deux
# plateformes » quand le workflow en construit trois, sans ligne macOS au
# tableau. Le contrôle accorde RELEASE.md au workflow, au metainfo et aux paquets.
run "docs : RELEASE.md, emplacements de version complets" "$ROOT" ./scripts/tests/release-emplacements-version-complets.sh
# SECURITY.md déclarait « 0.6.x » comme la dernière série supportée alors que le
# dépôt était en 0.9.2 (figé depuis la 0.6.2, sept versions plus tôt) : un
# rapporteur sur la 0.9.2 y lisait sa version comme non supportée. La section est
# désormais sans numéro (« la dernière version publiée ») ; le contrôle rougit si
# un numéro périmé face à Cargo.toml y réapparaît (hors rappel historique).
run "docs : SECURITY.md, versions supportées sans numéro périmé" "$ROOT" ./scripts/tests/securite-versions-supportees.sh
# docs/architecture.md dérivait du code sur trois points : la puce Distribution
# n'annonçait qu'AppImage + NSIS quand tauri.conf.json construit aussi deb/rpm et
# release.yml publie une archive portable et un .dmg macOS ; la phrase d'ouverture
# présentait « SSH et RDP » sans VNC ni port série, tous deux décrits plus bas ;
# la liste des add-ons xterm omettait `serialize`, déclaré dans web/package.json.
# Le contrôle lit ces trois vérités dans le code et exige que le doc les reprenne.
run "docs : architecture.md, distribution/périmètre/add-ons à jour" "$ROOT" ./scripts/tests/architecture-distribution-et-perimetre.sh
# La feuille de route (2.3) affirmait « Seules les sessions SSH sont reprises »,
# état antérieur au support des bureaux RDP MobaXterm : le code lit les signets
# `#91` (crates/avash/src/import.rs : BureauImporte, parse #91#) et les écrit dans
# le magasin rdphost, ce que le README annonce déjà. Le contrôle prend le code
# pour vérité et exige que la section 2.3 ne dise pas ces bureaux ignorés.
run "docs : feuille de route, import RDP MobaXterm décrit" "$ROOT" ./scripts/tests/feuille-de-route-import-rdp.sh
# La feuille de route se contredisait : « Comment savoir » affichait 69 scénarios
# et 75 %/66 % de couverture, quand « Où nous en sommes » donnait 71 et 84 %/81 %
# dans le même fichier. Le contrôle prend le décompte des `it(` et le relevé de
# docs/qualite.md pour vérité et exige que les deux tableaux les citent tous deux.
run "docs : feuille de route, tableaux chiffrés accordés" "$ROOT" ./scripts/tests/feuille-de-route-compteurs-coherents.sh
# `audit.toml` à la racine n'était lu par aucun outil : cargo-audit ne charge sa
# config que depuis .cargo/audit.toml. Le fichier racine était mort et la liste
# effective vivait dans les --ignore de check.sh et des deux CI, qui divergeaient
# (RUSTSEC-2024-0429 y figurait, pas dans audit.toml). Le contrôle exige la config
# centralisée dans .cargo/audit.toml, alignée sur deny.toml, sans --ignore résiduel.
run "audit : avis cargo-audit centralisés (.cargo/audit.toml)" "$ROOT" ./scripts/tests/audit-config-centralise.sh
# deny.toml (racine) prétendait que rsa n'était pas employé pour du RSA privé,
# alors qu'avash signe le défi SSH avec la clé id_rsa de l'utilisateur ; et
# rdp-sidecar/deny.toml recopiait ce texte en nommant russh, absent de l'arbre
# du sidecar (rsa y vient de picky, pour du X.509). Le contrôle exige que chaque
# justification de RUSTSEC-2023-0071 dise la vérité sur l'arbre qu'elle gouverne.
run "audit : justification RUSTSEC-2023-0071 (Marvin) exacte" "$ROOT" ./scripts/tests/deny-justification-rsa-marvin.sh
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
# `waitForPort` : deux connexions successives ne prouvent PAS le retour de la
# boucle d'acceptation (l'événement `connect` vient du backlog du noyau, sans
# accept() serveur) ; le commentaire et le README doivent le dire au lieu de
# promettre cette garantie fausse.
run "waitForPort : justification honnête (e2e)" "$ROOT" node --test scripts/tests/e2e-waitforport-justification.mjs
# Le sshd du harnais n'était jamais vérifié au démarrage : un sshd orphelin d'un
# run précédent tenant le port 2223 servait à la place du nôtre (mort en silence
# sur le bind), et toute la suite SSH échouait sans nommer le port occupé.
# onPrepare doit désormais attendre le port et exiger le fichier PID de notre
# sshd, et refuser un tauri-driver orphelin sur 4444.
run "sshd vérifié au démarrage (e2e)" "$ROOT" node --test scripts/tests/e2e-sshd-verifie-au-demarrage.mjs
# onComplete ne supprimait pas le bac à sable et plusieurs specs laissaient leur
# mkdtempSync dans /tmp : sur un tmpfs, chaque run consommait de la mémoire vive
# (clé privée cliente résiduelle, ~6 Mo de données par run) jusqu'au redémarrage.
run "nettoyage des temporaires (e2e)" "$ROOT" node --test scripts/tests/e2e-nettoyage-temporaires.mjs
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
# transitives (deepmerge-ts, serialize-javascript) portent des avis « haute »
# sans correctif en amont : ce code ne tourne que sur la machine de test, jamais
# chez l'utilisateur. On ne bloque que sur « critique ». Les trois overrides
# d'e2e/package.json imposent des versions saines (extract-zip, longtemps
# nommé ici, a quitté l'arbre depuis @puppeteer/browsers 3) — cf. la section
# « overrides » d'e2e/README.md.
run "audit npm (e2e)"    "$ROOT/e2e" "$ROOT/scripts/npm-audit.sh" critical tolerer-registre
run "tests"              "$WEB" npx vitest run
# Le build du front (`vite build`, web/dist) a lieu en tete, dans « Prerequis » :
# la section Rust en depend. Ici la section Front ne garde que lint/tests/audit.

if [ "$QUICK" != "--quick" ]; then
  step "Build release"
  # Le sidecar (externalBin) et web/dist (frontendDist) sont deja en place depuis
  # « Prerequis » : le build release final d'avash-ui n'a plus qu'a les embarquer.
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
