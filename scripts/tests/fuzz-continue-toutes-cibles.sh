#!/usr/bin/env bash
# Contrôle reproductible : fuzz/fuzz.sh doit jouer TOUTES les cibles et ne
# rougir qu'à la fin, en nommant chaque cible tombée.
#
# Trouvé le 8 septembre 2026 : le script sortait au premier plantage. La chaîne
# Sécurité de la 0.10.0 a rougi sur `config_ssh` et n'a jamais atteint les six
# autres cibles ; il a fallu les rejouer à la main pour savoir qu'elles étaient
# saines. Une campagne doit dire l'état de toutes les cibles en une passe.
#
# Trouvé par l'audit du 9 septembre 2026 : la liste des cibles était écrite en
# dur dans fuzz.sh et avait pris du retard sur `fuzz/Cargo.toml`. Deux cibles
# sur neuf (`glob_match_pur`, `osinfo`) n'étaient donc jamais fuzzées, seulement
# compilées sur les PR : un parseur couvert sur le papier, jamais secoué en
# fait. Ce test compte désormais les cibles DÉCLARÉES et exige qu'elles soient
# toutes jouées : ajouter une cible sans la jouer fait rougir la chaîne.
#
# On ne lance pas cargo-fuzz (nightly, minutes) : un faux `cargo` posé en tête du
# PATH échoue sur deux cibles et réussit sur les autres. Contre l'ancien script
# (sortie au premier échec), une seule cible aurait été jouée et une seule
# nommée : les deux assertions rougissaient.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

script="$PWD/fuzz/fuzz.sh"
echec=0
bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT
mkdir -p "$bac/fuzz" "$bac/bin"
cp "$script" "$bac/fuzz/fuzz.sh"
cp "$PWD/fuzz/Cargo.toml" "$bac/fuzz/Cargo.toml"

# Source de vérité : les cibles déclarées dans fuzz/Cargo.toml.
mapfile -t DECLAREES < <(sed -n '/^\[\[bin\]\]/,/^$/ s/^name = "\(.*\)"/\1/p' fuzz/Cargo.toml)
ATTENDU="${#DECLAREES[@]}"
if [ "$ATTENDU" -lt 2 ]; then
  echo "✗ fuzz-continue-toutes-cibles : aucune cible lue dans fuzz/Cargo.toml" >&2
  exit 1
fi
for c in "${DECLAREES[@]}"; do
  mkdir -p "$bac/fuzz/seeds/$c"
done

# Chaque cible déclarée a-t-elle bien ses graines commitées ?
for c in "${DECLAREES[@]}"; do
  if [ ! -d "fuzz/seeds/$c" ]; then
    echo "  ✗ la cible « $c » est déclarée mais n'a aucune graine dans fuzz/seeds/" >&2
    echec=1
  fi
done

# Faux cargo : `cargo +nightly fuzz run <cible> …` ; $4 est la cible.
faux_cargo() { # cibles_qui_plantent (séparées par des espaces)
  cat > "$bac/bin/cargo" <<EOF
#!/usr/bin/env bash
cible="\$4"
for p in $1; do
  if [ "\$cible" = "\$p" ]; then echo "==1== ERROR: libFuzzer: deadly signal"; exit 1; fi
done
echo "#100 DONE   cov: 1 ft: 1"
exit 0
EOF
  chmod +x "$bac/bin/cargo"
}

# Cas 1 : deux cibles plantent, à des positions différentes (la première
# déclarée et une du milieu), pour prouver que le script ne s'arrête pas.
premiere="${DECLAREES[0]}"
milieu="${DECLAREES[$((ATTENDU / 2))]}"
faux_cargo "$premiere $milieu"
sortie="$(cd "$bac" && PATH="$bac/bin:$PATH" DUREE=1 bash fuzz/fuzz.sh 2>&1)" && code=0 || code=$?
if [ "$code" -eq 0 ]; then
  echo "  ✗ fuzz.sh est resté vert alors que deux cibles plantaient" >&2; echec=1
fi
jouees="$(grep -c '^▸ fuzz : ' <<<"$sortie" || true)"
if [ "$jouees" -ne "$ATTENDU" ]; then
  echo "  ✗ $jouees cible(s) jouée(s) sur $ATTENDU déclarée(s) dans fuzz/Cargo.toml" >&2; echec=1
fi
# Chaque cible déclarée doit apparaître nommément dans la campagne.
for c in "${DECLAREES[@]}"; do
  if ! grep -q "^▸ fuzz : $c " <<<"$sortie"; then
    echo "  ✗ la cible « $c » est déclarée mais n'est jamais jouée" >&2; echec=1
  fi
done
if ! grep -qE "2 cible\(s\) sur $ATTENDU en échec : $premiere $milieu" <<<"$sortie"; then
  echo "  ✗ le bilan final ne nomme pas les deux cibles tombées" >&2
  echo "$sortie" | tail -3 | sed 's/^/      /' >&2; echec=1
fi

# Cas 2 : rien ne plante → vert, sept cibles.
faux_cargo ""
sortie="$(cd "$bac" && PATH="$bac/bin:$PATH" DUREE=1 bash fuzz/fuzz.sh 2>&1)" && code=0 || code=$?
if [ "$code" -ne 0 ] || ! grep -q "✓ fuzz : $ATTENDU cibles" <<<"$sortie"; then
  echo "  ✗ fuzz.sh doit rester vert quand aucune cible ne plante" >&2; echec=1
fi

if [ "$echec" -ne 0 ]; then
  echo "✗ fuzz-continue-toutes-cibles : fuzz.sh ne joue pas toutes les cibles." >&2
  exit 1
fi
echo "✓ fuzz-continue-toutes-cibles : les $ATTENDU cibles déclarées sont jouées, les échecs nommés à la fin"
