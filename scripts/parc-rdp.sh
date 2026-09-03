#!/usr/bin/env bash
# Parc RDP local : de vrais serveurs xrdp, en conteneur, pour éprouver le
# rendu et les entrées contre autre chose que des simulacres.
#
# Les trois défauts RDP de la version 0.3.3 — image cisaillée, clavier en
# QWERTY, connexion suspendue — ont tous été trouvés contre de vraies machines
# et AUCUN n'aurait été vu par les tests d'alors. Ce parc existe pour que la
# prochaine fois, ce soit la machine qui le dise, pas l'utilisateur.
#
#   scripts/parc-rdp.sh up [xfce|gnome|ssh|tous]   démarre
#   scripts/parc-rdp.sh down                   arrête et nettoie
#   scripts/parc-rdp.sh status                 état et ports
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

PORT_XFCE=3390
PORT_GNOME=3391
PORT_SSH=2222
# podman en local, docker sur les serveurs d'intégration : on prend ce qui est là
# plutôt que d'imposer l'un des deux.
MOTEUR="${MOTEUR:-$(command -v podman >/dev/null 2>&1 && echo podman || echo docker)}"
# Où joindre le parc. Sur un poste, le démon est local et les conteneurs
# publient sur la boucle locale. Sur GitLab, le job tourne dans un conteneur
# et pilote le démon de l'hôte par son socket : les conteneurs publient sur
# l'adresse du pont Docker (172.17.0.1), que le job joint et qui n'est pas
# exposée sur le réseau — le premier passage réel attendait 127.0.0.1:3390
# et n'y trouvait rien. Une adresse IPv4 sert donc aussi de liaison ; un nom
# d'hôte (démon distant, docker-in-docker) impose de publier sur toutes les
# interfaces de ce démon. Les contrôles de conformite.sh lisent la même
# variable.
PARC_HOTE="${PARC_HOTE:-127.0.0.1}"
case "$PARC_HOTE" in
  *[!0-9.]*) LIAISON="0.0.0.0" ;;
  *) LIAISON="$PARC_HOTE" ;;
esac
# Mode réseau : PARC_RESEAU nomme un réseau Docker dédié. Les conteneurs du parc
# y tournent sans publier de port, et l'appelant, s'il est lui-même un
# conteneur (le job GitLab, qui pilote le démon de l'hôte par son socket), s'y
# raccorde pour les joindre par leur adresse sur ce réseau. Pourquoi pas
# publier sur le pont de l'hôte : depuis un conteneur, un port publié sur
# 172.17.0.1 ne répond pas — les règles de Docker ne traduisent pas le trafic
# qui vient du pont lui-même — vu en répétition. Et rien n'est exposé sur le
# réseau du poste. `cible <nom>` rend « hôte port » dans l'un ou l'autre mode :
# c'est ce que conformite.sh interroge.
PARC_RESEAU="${PARC_RESEAU:-}"

adresse() { # nom → adresse du conteneur sur PARC_RESEAU
  $MOTEUR inspect -f "{{(index .NetworkSettings.Networks \"$PARC_RESEAU\").IPAddress}}" "avash-parc-$1"
}

port_interne() { case "$1" in ssh) echo 22 ;; *) echo 3389 ;; esac; }
port_publie()  { case "$1" in xfce) echo "$PORT_XFCE" ;; gnome) echo "$PORT_GNOME" ;; ssh) echo "$PORT_SSH" ;; esac; }

demarrer() { # nom
  local nom="avash-parc-$1" interne cf="tests-parc/Containerfile.$1"
  interne="$(port_interne "$1")"
  if $MOTEUR image inspect "$nom" >/dev/null 2>&1; then
    echo "  image $nom déjà construite"
  else
    echo "  construction de $nom (long la première fois)…"
    $MOTEUR build -q -t "$nom" -f "$cf" tests-parc >/dev/null
  fi
  $MOTEUR rm -f "$nom" >/dev/null 2>&1 || true
  if [ -n "$PARC_RESEAU" ]; then
    $MOTEUR network inspect "$PARC_RESEAU" >/dev/null 2>&1 || $MOTEUR network create "$PARC_RESEAU" >/dev/null
    $MOTEUR run -d --name "$nom" --network "$PARC_RESEAU" "$nom" >/dev/null
    echo "  $nom écoute sur $(adresse "$1"):$interne (réseau $PARC_RESEAU)"
  else
    $MOTEUR run -d --name "$nom" -p "$LIAISON:$(port_publie "$1"):$interne" "$nom" >/dev/null
    echo "  $nom écoute sur $PARC_HOTE:$(port_publie "$1")"
  fi
}

raccorder() { # l'appelant est un conteneur : le raccorder au réseau du parc
  [ -n "$PARC_RESEAU" ] && [ -f /.dockerenv ] || return 0
  # L'identifiant du conteneur, pas son nom d'hôte : l'exécuteur GitLab donne
  # aux siens un nom d'hôte qui n'est pas leur identifiant, et « network
  # connect » ne le connaissait pas — l'échec, avalé, laissait l'attente
  # pendre une heure sur une adresse injoignable (job 31269). Le montage de
  # /etc/hostname porte l'identifiant.
  local moi
  moi=$(grep -o -m1 'containers/[0-9a-f]\{64\}' /proc/self/mountinfo 2>/dev/null | cut -d/ -f2)
  [ -n "$moi" ] || moi="$(hostname)"
  if $MOTEUR network inspect "$PARC_RESEAU" --format '{{range .Containers}}{{.Name}} {{end}}' 2>/dev/null | grep -q "$moi"; then
    return 0
  fi
  $MOTEUR network connect "$PARC_RESEAU" "$moi" >/dev/null 2>&1 \
    || echo "  ✗ raccordement de ce conteneur ($moi) au réseau $PARC_RESEAU impossible" >&2
}

attendre() { # hôte port
  # Deux secondes par essai : une adresse injoignable ne doit pas faire
  # attendre le délai de connexion TCP (des minutes) à chaque tour.
  for _ in $(seq 1 30); do
    timeout 2 bash -c "exec 3<>/dev/tcp/$1/$2" 2>/dev/null && return 0
    sleep 1
  done
  echo "  ✗ rien n'écoute sur $1:$2" >&2; return 1
}

cible() { # nom → « hôte port » que les contrôles doivent joindre
  if [ -n "$PARC_RESEAU" ]; then echo "$(adresse "$1") $(port_interne "$1")"; else echo "$PARC_HOTE $(port_publie "$1")"; fi
}

lancer() { # nom
  demarrer "$1"
  raccorder
  # shellcheck disable=SC2046
  attendre $(cible "$1")
}

case "${1:-status}" in
  up)
    quoi="${2:-xfce}"
    [ "$quoi" = "xfce"  ] || [ "$quoi" = "tous" ] && lancer xfce
    [ "$quoi" = "gnome" ] || [ "$quoi" = "tous" ] && lancer gnome
    [ "$quoi" = "ssh"   ] || [ "$quoi" = "tous" ] && lancer ssh
    echo "✓ parc prêt (compte « essai », mot de passe « essai-mot-de-passe »)"
    ;;
  down)
    for n in avash-parc-xfce avash-parc-gnome avash-parc-ssh; do $MOTEUR rm -f "$n" >/dev/null 2>&1 || true; done
    if [ -n "$PARC_RESEAU" ]; then
      [ -f /.dockerenv ] && $MOTEUR network disconnect "$PARC_RESEAU" "$(hostname)" >/dev/null 2>&1 || true
      $MOTEUR network rm "$PARC_RESEAU" >/dev/null 2>&1 || true
    fi
    echo "✓ parc arrêté"
    ;;
  status)
    $MOTEUR ps --filter name=avash-parc --format "  {{.Names}}\t{{.Status}}\t{{.Ports}}" 2>/dev/null || true
    ;;
  cible)
    case "${2:-}" in xfce|gnome|ssh) cible "$2" ;; *) echo "usage : $0 cible {xfce|gnome|ssh}" >&2; exit 2 ;; esac
    ;;
  *) echo "usage : $0 {up [xfce|gnome|ssh|tous]|down|status|cible <nom>}" >&2; exit 2 ;;
esac
