#!/usr/bin/env bash
# Contrôle reproductible : fuzz/fuzz.sh doit jouer TOUTES les cibles et ne
# rougir qu'à la fin, en nommant chaque cible tombée.
#
# Trouvé le 8 septembre 2026 : le script sortait au premier plantage. La chaîne
# Sécurité de la 0.10.0 a rougi sur `config_ssh` et n'a jamais atteint les six
# autres cibles ; il a fallu les rejouer à la main pour savoir qu'elles étaient
# saines. Une campagne doit dire l'état de toutes les cibles en une passe.
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
for c in config_ssh putty_session reg_query mobaxterm_ini asciicast clearcodec vnc_serveur; do
  mkdir -p "$bac/fuzz/seeds/$c"
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

# Cas 1 : deux cibles sur sept plantent, à des positions différentes.
faux_cargo "config_ssh asciicast"
sortie="$(cd "$bac" && PATH="$bac/bin:$PATH" DUREE=1 bash fuzz/fuzz.sh 2>&1)" && code=0 || code=$?
if [ "$code" -eq 0 ]; then
  echo "  ✗ fuzz.sh est resté vert alors que deux cibles plantaient" >&2; echec=1
fi
jouees="$(grep -c '^▸ fuzz : ' <<<"$sortie" || true)"
if [ "$jouees" -ne 7 ]; then
  echo "  ✗ $jouees cible(s) jouée(s) sur 7 : le script s'arrête avant la fin" >&2; echec=1
fi
if ! grep -qE '2 cible\(s\) sur 7 en échec : config_ssh asciicast' <<<"$sortie"; then
  echo "  ✗ le bilan final ne nomme pas les deux cibles tombées" >&2
  echo "$sortie" | tail -3 | sed 's/^/      /' >&2; echec=1
fi

# Cas 2 : rien ne plante → vert, sept cibles.
faux_cargo ""
sortie="$(cd "$bac" && PATH="$bac/bin:$PATH" DUREE=1 bash fuzz/fuzz.sh 2>&1)" && code=0 || code=$?
if [ "$code" -ne 0 ] || ! grep -q '✓ fuzz : 7 cibles' <<<"$sortie"; then
  echo "  ✗ fuzz.sh doit rester vert quand aucune cible ne plante" >&2; echec=1
fi

if [ "$echec" -ne 0 ]; then
  echo "✗ fuzz-continue-toutes-cibles : fuzz.sh ne joue pas toutes les cibles." >&2
  exit 1
fi
echo "✓ fuzz-continue-toutes-cibles : les 7 cibles sont jouées, les échecs nommés à la fin"
