#!/usr/bin/env bash
# Joue chaque cible de fuzzing pendant DUREE secondes (60 par défaut), depuis
# les graines commises (`seeds/`) vers un corpus local (`corpus/`, ignoré par
# git). Sort en erreur au premier plantage : l'entrée fautive est dans
# `artifacts/<cible>/`, rejouable avec `cargo +nightly fuzz run <cible> <fichier>`.
set -euo pipefail
cd "$(dirname "$0")"
DUREE="${DUREE:-60}"
CIBLES=(config_ssh putty_session reg_query mobaxterm_ini asciicast clearcodec vnc_serveur)
for c in "${CIBLES[@]}"; do
  mkdir -p "corpus/$c"
  echo "▸ fuzz : $c (${DUREE}s)"
  cargo +nightly fuzz run "$c" "corpus/$c" "seeds/$c" -- \
    -max_total_time="$DUREE" -timeout=10 -max_len=65536 -print_final_stats=1 2>&1 \
    | grep -E "^(#[0-9]+.*(DONE|NEW|pulse)|==.*ERROR|.*panicked|SUMMARY|stat::|▸|Running|Failing|Output of)" \
    | tail -12
done
echo "✓ fuzz : ${#CIBLES[@]} cibles, aucune entrée fautive"
