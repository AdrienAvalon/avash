#!/usr/bin/env bash
# Met de côté les profils de couverture (.profraw) que llvm-profdata refuse
# pris un à un, pour que le rapport fusionne tous les autres.
#
# Pourquoi : le pilote WebDriver tue l'application sans sortie propre, et le
# fil `cfg(coverage)` des deux `main` réécrit le profil chaque seconde (voir
# scripts/couverture.sh). Un arrêt tombé pendant une écriture laisse un profil
# tronqué ou incohérent. Vu au workflow qualité le 16 octobre 2026 : deux
# profils de l'application à compteur 2^64 - 1 (« counter mismatch »), et
# llvm-profdata, qui refuse toute la fusion dès qu'une entrée est invalide,
# rendait « no profile can be merged » après 21 minutes d'une suite verte.
# Écarter un profil abîmé ne coûte que la dernière seconde d'un processus.
#
# Usage : scripts/ecarter-profils-abimes.sh <dossier> [<dossier>...]
# Les profils écartés vont dans <dossier>/profils-ecartes/ (gardés pour
# l'examen) ; le script nomme chacun et sort en erreur si llvm-profdata est
# introuvable ou s'il ne reste plus aucun profil.
set -euo pipefail

[ "$#" -ge 1 ] || { echo "usage : $0 <dossier> [<dossier>...]" >&2; exit 2; }

# llvm-profdata du composant llvm-tools de la toolchain active : celui
# qu'appelle cargo-llvm-cov, donc du même format de profil.
profdata=""
for candidat in "$(rustc --print sysroot)"/lib/rustlib/*/bin/llvm-profdata; do
  [ -x "$candidat" ] && profdata="$candidat" && break
done
[ -n "$profdata" ] || { echo "llvm-profdata introuvable (rustup component add llvm-tools)" >&2; exit 1; }

for dossier in "$@"; do
  gardes=0
  ecartes=0
  for p in "$dossier"/*.profraw; do
    [ -e "$p" ] || continue
    if "$profdata" merge -sparse "$p" -o /dev/null >/dev/null 2>&1; then
      gardes=$((gardes + 1))
    else
      mkdir -p "$dossier/profils-ecartes"
      mv "$p" "$dossier/profils-ecartes/"
      echo "écarté (refusé par llvm-profdata) : $p"
      ecartes=$((ecartes + 1))
    fi
  done
  echo "$dossier : $gardes profil(s) gardé(s), $ecartes écarté(s)"
  [ "$gardes" -gt 0 ] || { echo "aucun profil utilisable dans $dossier" >&2; exit 1; }
done
