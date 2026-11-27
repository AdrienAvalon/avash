#!/usr/bin/env bash
# Joue chaque cible de fuzzing pendant DUREE secondes (60 par défaut), depuis
# les graines commises (`seeds/`) vers un corpus local (`corpus/`, ignoré par
# git). Joue TOUTES les cibles, puis sort en erreur s'il y a eu au moins un
# plantage : l'entrée fautive est dans `artifacts/<cible>/`, rejouable avec
# `cargo +nightly fuzz run <cible> <fichier>`.
#
# Trouvé le 8 septembre 2026 : le script sortait au PREMIER plantage. La chaîne
# Sécurité de la 0.10.0 a rougi sur `config_ssh`… et n'a jamais atteint les six
# autres cibles, dont on ne savait donc rien tant que la première n'était pas
# réparée. Une campagne doit dire l'état de TOUTES les cibles en une passe.
set -uo pipefail
cd "$(dirname "$0")"
DUREE="${DUREE:-60}"
# Les cibles se lisent dans Cargo.toml, qui les déclare déjà : une liste écrite
# en dur ici avait pris du retard sur lui, et `glob_match_pur` comme `osinfo`
# n'étaient jamais fuzzées, seulement compilées sur les PR, donc couvertes sur
# le papier et jamais secouées en fait. Trouvé par l'audit du 9 septembre 2026.
#
# Deux crates : `fuzz/` (le cœur) et `fuzz/rdp/` (le processus RDP, par sa
# bibliothèque). Le cœur et IronRDP ne tiennent pas dans un même graphe de
# dépendances (audit du 27 novembre 2026, QT-11, voir rdp/Cargo.toml) ; les
# graines et le corpus restent ici, sous le nom de la cible.
CRATES=(. rdp)
CIBLES=()
DOSSIERS=()
for crate in "${CRATES[@]}"; do
  mapfile -t lues < <(sed -n '/^\[\[bin\]\]/,/^$/ s/^name = "\(.*\)"/\1/p' "$crate/Cargo.toml")
  if [ "${#lues[@]}" -eq 0 ]; then
    echo "✗ fuzz : aucune cible lue dans fuzz/$crate/Cargo.toml" >&2
    exit 1
  fi
  for c in "${lues[@]}"; do
    CIBLES+=("$c")
    DOSSIERS+=("$crate")
  done
  # `cargo fuzz` (0.13) n'a pas d'option `--locked` : on vérifie le verrou
  # avant la campagne, comme le font toutes les commandes cargo des chaînes
  # depuis l'audit du 12 septembre 2026. Un Cargo.lock périmé face à la
  # version d'avash se voyait sinon réécrit en silence par la première cible.
  if ! (cd "$crate" && cargo metadata --locked --format-version 1 >/dev/null); then
    echo "✗ fuzz : fuzz/$crate/Cargo.lock n'est plus à jour (cargo update -p avash ou -p avash-rdp)" >&2
    exit 1
  fi
done
journal="$(mktemp)"
trap 'rm -f "$journal"' EXIT
echecs=()
for i in "${!CIBLES[@]}"; do
  c="${CIBLES[$i]}"
  # `--fuzz-dir` après les chemins : la cible reste le premier argument de
  # `fuzz run`, comme le lit la garde scripts/tests/fuzz-continue-toutes-cibles.sh.
  dossier=()
  [ "${DOSSIERS[$i]}" = "." ] || dossier=(--fuzz-dir "${DOSSIERS[$i]}")
  mkdir -p "corpus/$c"
  echo "▸ fuzz : $c (${DUREE}s)"
  # La sortie brute va dans un journal ; on n'en affiche que l'essentiel. Sur
  # échec, la fin du journal brut est déversée (une erreur de toolchain ou de
  # compilation ne laissait sinon aucune ligne pour dire pourquoi), puis on
  # PASSE À LA CIBLE SUIVANTE au lieu de sortir.
  if ! cargo +nightly fuzz run "$c" "corpus/$c" "seeds/$c" "${dossier[@]}" -- \
      -max_total_time="$DUREE" -timeout=10 -max_len=65536 -print_final_stats=1 \
      >"$journal" 2>&1; then
    grep -E "^(#[0-9]+.*(DONE|NEW|pulse)|==.*ERROR|.*panicked|SUMMARY|stat::|▸|Running|Failing|Output of)" \
      "$journal" | tail -12 || true
    echo "✗ fuzz : « $c » a échoué — fin du journal brut :" >&2
    tail -60 "$journal" >&2
    echecs+=("$c")
    continue
  fi
  grep -E "^(#[0-9]+.*(DONE|NEW|pulse)|SUMMARY|stat::)" "$journal" | tail -12 || true
done
if [ "${#echecs[@]}" -ne 0 ]; then
  echo "✗ fuzz : ${#echecs[@]} cible(s) sur ${#CIBLES[@]} en échec : ${echecs[*]}" >&2
  exit 1
fi
echo "✓ fuzz : ${#CIBLES[@]} cibles, aucune entrée fautive"
