#!/usr/bin/env bash
# Conformité contre de VRAIS serveurs (parc local en conteneur).
#
# Chacun de ces contrôles correspond à un défaut réellement rencontré, signalé
# par l'usage et non par les tests. Ils existent pour que ce ne soit plus le cas.
#
# RDP :
#   1. la connexion aboutit          — elle restait suspendue sans fin (autodetect)
#   2. l'image n'est pas cisaillée   — le décodage glissait d'une ligne à l'autre
#   3. le clavier annoncé est le bon — xrdp retombait sur du QWERTY
# SSH :
#   4. le repli clavier-interactif   — un compte de domaine ne pouvait pas se
#      connecter, faute de ce repli ; signalé depuis Windows, pas par les tests
#   5. SFTP de bout en bout        — dépôt, relecture à l'octet près, effacement
#
# Usage : scripts/conformite.sh [xfce|gnome|ssh|tous]
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
  # 4. Le trafic de trames : les zones modifiées doivent rester séparées.
  #
  # Le processus n'accumulait qu'une union englobante — deux poussières aux
  # coins opposés donnaient un plein écran, soit le double d'octets mesuré sur
  # le fil. Le contrôle ne fixe pas de seuil en octets, qui serait fragile : il
  # vérifie qu'une trame porte PLUSIEURS rectangles. Un retour à l'union
  # englobante donnerait exactement autant de rectangles que de trames.
  if python3 -c "import websockets" 2>/dev/null; then
    local mesure t_ r_ o_
    if mesure=$(python3 tests-parc/mesure-trames.py "$port" 12 2>/dev/null); then
      read -r t_ r_ o_ <<<"$mesure"
      if [ "$r_" -gt "$t_" ]; then
        vert "trames : $t_ pour $r_ rectangles, $(( o_ / 1000 )) Ko ($(( o_ / t_ / 1000 )) Ko/trame)"
      else
        rouge "chaque trame ne porte qu'un rectangle ($t_ trames, $r_ rectangles) : retour à l'union englobante ?"
      fi
    else
      rouge "mesure de trames impossible"
    fi
  else
    printf '  \033[33m~\033[0m %s\n' "trafic de trames (python-websockets absent)"
  fi

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

eprouver_ssh() { # port
  echo "▸ ssh (127.0.0.1:$1)"
  # Ce serveur REFUSE la méthode « password » : seul le repli peut aboutir.
  cargo run -q -p avash --example ssh_conformite -- "$1" essai 'essai-mot-de-passe' \
    || echecs=$((echecs+1))
  # SFTP contre un VRAI OpenSSH : les tests d'intégration parlent à un serveur
  # monté en mémoire, c'est-à-dire à notre propre compréhension du protocole.
  cargo run -q -p avash --example sftp_conformite -- "$1" essai 'essai-mot-de-passe' \
    || echecs=$((echecs+1))
}

quoi="${1:-xfce}"
[ "$quoi" = "xfce"  ] || [ "$quoi" = "tous" ] && eprouver xfce  3390
[ "$quoi" = "gnome" ] || [ "$quoi" = "tous" ] && eprouver gnome 3391
[ "$quoi" = "ssh"   ] || [ "$quoi" = "tous" ] && eprouver_ssh 2222

echo
if [ "$echecs" -eq 0 ]; then
  printf '\033[1;32m✓ Conformité : tout est vert.\033[0m\n'
else
  printf '\033[1;31m✗ Conformité : %d contrôle(s) en échec.\033[0m\n' "$echecs"; exit 1
fi
