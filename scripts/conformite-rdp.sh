#!/usr/bin/env bash
# Conformité RDP contre de VRAIS serveurs xrdp (parc local en conteneur).
#
# Chacun de ces contrôles correspond à un défaut réellement rencontré, signalé
# par l'usage et non par les tests. Ils existent pour que ce ne soit plus le cas.
#
#   1. la connexion aboutit          — elle restait suspendue sans fin (autodetect)
#   2. l'image n'est pas cisaillée   — le décodage glissait d'une ligne à l'autre
#   3. le clavier annoncé est le bon — xrdp retombait sur du QWERTY
#
# Usage : scripts/conformite-rdp.sh [xfce|gnome|tous]
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

RDP="rdp-sidecar/target/release/avash-rdp"
COMPTE="essai"; MDP="essai-mot-de-passe"
DELAI=45
SORTIE="$(mktemp -d)"; trap 'rm -rf "$SORTIE"' EXIT
echecs=0

vert()  { printf '  \033[32m✓\033[0m %s\n' "$1"; }
rouge() { printf '  \033[31m✗\033[0m %s\n' "$1"; echecs=$((echecs+1)); }

eprouver() { # nom  port
  local nom="$1" port="$2" img="$SORTIE/$1.png" journal="$SORTIE/$1.log"
  echo "▸ $nom (127.0.0.1:$port)"

  # 1. La connexion aboutit, et on mesure en combien de temps.
  local t0 t1
  t0=$(date +%s%N)
  if AVASH_RDP_TRACE=ironrdp_connector=debug timeout "$DELAI" "$RDP" \
       --host 127.0.0.1 --port "$port" -u "$COMPTE" -p "$MDP" --sans-nla \
       --width 1024 --height 768 --shot "$img" >"$journal" 2>&1; then
    t1=$(date +%s%N)
    vert "connexion aboutie en $(( (t1 - t0) / 1000000 )) ms"
  else
    rouge "connexion échouée ou expirée (voir $journal)"
    sed -n '$p' "$journal" | sed 's/^/      /'
    return
  fi

  # 2. L'image rendue n'est pas cisaillée.
  if python3 tests-parc/detecteur-cisaillement.py "$img" >"$SORTIE/$1.detect" 2>&1; then
    vert "image saine $(sed 's/.*décalage/(décalage/;s/ .*png//' "$SORTIE/$1.detect" | tr -d '\n'))"
  else
    rouge "image cisaillée"
    sed 's/^/      /' "$SORTIE/$1.detect"
  fi

  # 3. La disposition clavier annoncée est celle du poste, pas 0.
  # Viser la ligne « Send ConnectInitial » : c'est CELLE qu'on envoie. Les
  # capacités échangées ensuite contiennent d'autres champs du même nom, à zéro,
  # qui feraient conclure à tort.
  local annoncee
  annoncee=$(grep "Send ConnectInitial" "$journal" | grep -oE "keyboard_layout: [0-9]+" | head -1 | cut -d' ' -f2)
  if [ -z "$annoncee" ]; then
    rouge "disposition clavier introuvable dans la trace"
  elif [ "$annoncee" = "0" ]; then
    rouge "disposition clavier annoncée = 0 (le serveur retombera sur QWERTY)"
  else
    vert "disposition clavier annoncée : $annoncee"
  fi
}

quoi="${1:-xfce}"
[ "$quoi" = "xfce"  ] || [ "$quoi" = "tous" ] && eprouver xfce  3390
[ "$quoi" = "gnome" ] || [ "$quoi" = "tous" ] && eprouver gnome 3391

echo
if [ "$echecs" -eq 0 ]; then
  printf '\033[1;32m✓ Conformité RDP : tout est vert.\033[0m\n'
else
  printf '\033[1;31m✗ Conformité RDP : %d contrôle(s) en échec.\033[0m\n' "$echecs"; exit 1
fi
