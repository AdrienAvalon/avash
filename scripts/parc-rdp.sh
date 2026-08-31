#!/usr/bin/env bash
# Parc RDP local : de vrais serveurs xrdp, en conteneur, pour éprouver le
# rendu et les entrées contre autre chose que des simulacres.
#
# Les trois défauts RDP de la version 0.3.3 — image cisaillée, clavier en
# QWERTY, connexion suspendue — ont tous été trouvés contre de vraies machines
# et AUCUN n'aurait été vu par les tests d'alors. Ce parc existe pour que la
# prochaine fois, ce soit la machine qui le dise, pas l'utilisateur.
#
#   scripts/parc-rdp.sh up [xfce|gnome|tous]   démarre
#   scripts/parc-rdp.sh down                   arrête et nettoie
#   scripts/parc-rdp.sh status                 état et ports
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

PORT_XFCE=3390
PORT_GNOME=3391
# podman en local, docker sur les serveurs d'intégration : on prend ce qui est là
# plutôt que d'imposer l'un des deux.
MOTEUR="${MOTEUR:-$(command -v podman >/dev/null 2>&1 && echo podman || echo docker)}"

demarrer() { # nom  port  fichier
  local nom="avash-parc-$1" port="$2" cf="tests-parc/Containerfile.$1"
  if $MOTEUR image inspect "$nom" >/dev/null 2>&1; then
    echo "  image $nom déjà construite"
  else
    echo "  construction de $nom (long la première fois)…"
    $MOTEUR build -q -t "$nom" -f "$cf" tests-parc >/dev/null
  fi
  $MOTEUR rm -f "$nom" >/dev/null 2>&1 || true
  $MOTEUR run -d --name "$nom" -p "127.0.0.1:$port:3389" "$nom" >/dev/null
  echo "  $nom écoute sur 127.0.0.1:$port"
}

attendre() { # port
  for _ in $(seq 1 60); do
    (exec 3<>"/dev/tcp/127.0.0.1/$1") 2>/dev/null && { exec 3<&-; return 0; }
    sleep 1
  done
  echo "  ✗ rien n'écoute sur $1" >&2; return 1
}

case "${1:-status}" in
  up)
    quoi="${2:-xfce}"
    [ "$quoi" = "xfce"  ] || [ "$quoi" = "tous" ] && { demarrer xfce  "$PORT_XFCE";  attendre "$PORT_XFCE"; }
    [ "$quoi" = "gnome" ] || [ "$quoi" = "tous" ] && { demarrer gnome "$PORT_GNOME"; attendre "$PORT_GNOME"; }
    echo "✓ parc prêt (compte « essai », mot de passe « essai-mot-de-passe »)"
    ;;
  down)
    for n in avash-parc-xfce avash-parc-gnome; do $MOTEUR rm -f "$n" >/dev/null 2>&1 || true; done
    echo "✓ parc arrêté"
    ;;
  status)
    $MOTEUR ps --filter name=avash-parc --format "  {{.Names}}\t{{.Status}}\t{{.Ports}}" 2>/dev/null || true
    ;;
  *) echo "usage : $0 {up [xfce|gnome|tous]|down|status}" >&2; exit 2 ;;
esac
