#!/usr/bin/env bash
# Couverture réelle du code : tests unitaires ET suite bout en bout, mesurés
# ensemble sur des binaires instrumentés.
#
# Pourquoi : `cargo llvm-cov` seul ne voit que les tests unitaires. Les
# commandes Tauri (qui exigent une fenêtre) et la boucle de session du
# processus RDP (qui exige un serveur) sont exercées par les 69 scénarios bout
# en bout sur la vraie application, et restaient à 0 % dans le rapport : le
# 76 % de l'espace de travail du 05/09/2026 cachait un cœur à 91 % et une
# interface à 48 % dont la moitié était en fait testée. Ici, l'application et
# le processus RDP sont construits instrumentés (release : le binaire embarque
# web/dist, et c'est celui que la suite pilote), la suite tourne dessus, et le
# rapport fusionne tout.
#
# Deux détails qui ont coûté : `strip = true` du profil release retirerait la
# table de couverture du binaire, d'où CARGO_PROFILE_RELEASE_STRIP=none ; et
# le pilote WebDriver arrête l'application sans sortie propre, d'où le fil
# `cfg(coverage)` des deux `main` qui réécrit le profil chaque seconde.
#
# Usage : scripts/couverture.sh [dossier de sortie]   (défaut : couverture/)
# Prérequis : ceux de la suite bout en bout (e2e/README.md), cargo-llvm-cov,
# llvm-tools, web/dist construit, serveurs de test construits en release.
set -euo pipefail

racine=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
sortie=$(realpath -m "${1:-$racine/couverture}")
mkdir -p "$sortie"
cd "$racine"

titre() { printf '\n\033[1;36m▸ %s\033[0m\n' "$*"; }

# L'environnement d'instrumentation de l'espace de travail : RUSTC_WRAPPER,
# LLVM_PROFILE_FILE (target/avash-<pid>-<empreinte>.profraw), cfg(coverage).
titre "Environnement d'instrumentation"
eval "$(cargo llvm-cov show-env --sh)"
export CARGO_PROFILE_RELEASE_STRIP=none
cargo llvm-cov clean --workspace

# Le processus RDP est un espace de travail à part : même recette, dans un
# sous-shell qui repart d'un environnement vierge. Un show-env imbriqué dans
# celui de l'espace de travail produit un environnement cassé (« nested
# show-env may not work correctly », puis « Cannot allocate memory » au premier
# appel du wrapper rustc, vu au premier passage du 06/09/2026).
sidecar() {
  (
    cd rdp-sidecar
    unset RUSTC_WRAPPER CARGO_LLVM_COV CARGO_LLVM_COV_SHOW_ENV CARGO_LLVM_COV_TARGET_DIR \
      CARGO_LLVM_COV_BUILD_DIR LLVM_PROFILE_FILE __CARGO_LLVM_COV_RUSTC_WRAPPER \
      __CARGO_LLVM_COV_RUSTC_WRAPPER_RUSTFLAGS __CARGO_LLVM_COV_RUSTC_WRAPPER_CRATE_NAMES
    eval "$(cargo llvm-cov show-env --sh)"
    export CARGO_PROFILE_RELEASE_STRIP=none
    "$@"
  )
}

titre "Binaires instrumentés (application et processus RDP, release)"
sidecar cargo llvm-cov clean
sidecar cargo build --locked --release
# Le build-script de l'application exige le binaire du sidecar sous
# binaries/<triplet> (ressource embarquée) : c'est l'instrumenté qu'on y pose.
triplet=$(rustc -vV | sed -n 's/^host: //p')
mkdir -p crates/avash-ui/binaries
cp rdp-sidecar/target/release/avash-rdp "crates/avash-ui/binaries/avash-rdp-$triplet"
cargo build --locked --release -p avash-ui

titre "Tests unitaires, même profil"
cargo test --locked --workspace --release
sidecar cargo test --locked --release

# La suite hérite de LLVM_PROFILE_FILE : l'application et le processus RDP
# qu'elle lance écrivent leurs profils dans target/ à côté de ceux des tests.
# En release, l'application ne cherche le sidecar qu'à côté de son exécutable
# (la copie que tauri-build y fait) ou dans AVASH_RDP_BIN : on nomme
# l'instrumenté explicitement plutôt que de dépendre de cette copie.
titre "Suite bout en bout sur les binaires instrumentés"
AVASH_RDP_BIN="$racine/rdp-sidecar/target/release/avash-rdp" \
  bash -c 'cd e2e && xvfb-run -a npm test'

titre "Rapport : espace de travail (cœur, interface)"
cargo llvm-cov report --release --summary-only | tee "$sortie/espace-de-travail.txt"
cargo llvm-cov report --release --html --output-dir "$sortie/espace-de-travail"

# Les profils que le processus RDP a écrits pendant la suite portent le nom de
# l'espace de travail (il a hérité de son LLVM_PROFILE_FILE) : on les recopie
# sous le préfixe du sidecar pour que son rapport les prenne. Les compteurs
# étrangers à ses objets sont ignorés à la fusion.
titre "Rapport : processus RDP"
n=0
for p in target/avash-*.profraw; do
  n=$((n + 1))
  cp "$p" "rdp-sidecar/target/avash-rdp-e2e-$n.profraw"
done
sidecar cargo llvm-cov report --release --summary-only | tee "$sortie/processus-rdp.txt"
sidecar cargo llvm-cov report --release --html --output-dir "$sortie/processus-rdp"

titre "Résumé"
for f in espace-de-travail processus-rdp; do
  printf '%-20s %s\n' "$f" "$(awk '/^TOTAL/ {print "lignes " $10 ", fonctions " $7 ", régions " $4}' "$sortie/$f.txt")"
done
echo "Rapports HTML dans $sortie/"
