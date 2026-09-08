#!/usr/bin/env bash
# Joue chaque cible de fuzzing pendant DUREE secondes (60 par défaut), depuis
# les graines commises (`seeds/`) vers un corpus local (`corpus/`, ignoré par
# git). Sort en erreur au premier plantage : l'entrée fautive est dans
# `artifacts/<cible>/`, rejouable avec `cargo +nightly fuzz run <cible> <fichier>`.
set -euo pipefail
cd "$(dirname "$0")"
DUREE="${DUREE:-60}"
CIBLES=(config_ssh putty_session reg_query mobaxterm_ini asciicast clearcodec vnc_serveur)
journal="$(mktemp)"
trap 'rm -f "$journal"' EXIT
for c in "${CIBLES[@]}"; do
  mkdir -p "corpus/$c"
  echo "▸ fuzz : $c (${DUREE}s)"
  # On écrit la sortie brute dans un journal et on n'en affiche que l'essentiel.
  # Trouvé par l'audit du 8 septembre 2026 : un `| grep | tail` filtrait TOUT
  # quand cargo échouait avant le fuzzing (toolchain nightly absente, cible qui
  # ne compile pas) — l'étape sortait en erreur sans une ligne pour dire
  # pourquoi. Sur échec, on déverse la fin du journal brut avant de sortir.
  if ! cargo +nightly fuzz run "$c" "corpus/$c" "seeds/$c" -- \
      -max_total_time="$DUREE" -timeout=10 -max_len=65536 -print_final_stats=1 \
      >"$journal" 2>&1; then
    grep -E "^(#[0-9]+.*(DONE|NEW|pulse)|==.*ERROR|.*panicked|SUMMARY|stat::|▸|Running|Failing|Output of)" \
      "$journal" | tail -12 || true
    echo "✗ fuzz : « $c » a échoué — fin du journal brut :" >&2
    tail -60 "$journal" >&2
    exit 1
  fi
  grep -E "^(#[0-9]+.*(DONE|NEW|pulse)|SUMMARY|stat::)" "$journal" | tail -12 || true
done
echo "✓ fuzz : ${#CIBLES[@]} cibles, aucune entrée fautive"
