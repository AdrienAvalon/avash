#!/usr/bin/env bash
# Capture une session RDP et la DÉCHIFFRE, PDU par PDU.
#
# RDP est chiffré dès la négociation : tcpdump et tshark ne montrent que du TLS,
# ce qui les rend inutiles pour comprendre un dialogue. Mais la pile TLS du
# processus RDP honore `SSLKEYLOGFILE` — capacité présente depuis toujours, que
# personne n'avait employée. Avec les clés, tshark nomme chaque PDU :
#
#   T.125  erectDomainRequest / attachUserConfirm / channelJoinConfirm 1003
#   RDP    ServerData Encryption: None
#
# C'est le complément du magnétoscope : celui-ci rejoue ce qu'on a compris,
# celui-là montre ce qui passe réellement sur le fil, en-têtes compris.
#
# ATTENTION : le fichier de clés déchiffre TOUTE la session, y compris l'échange
# CredSSP qui porte le mot de passe. Il est écrit dans un répertoire temporaire
# privé et effacé à la fin ; ne le conservez pas, ne le joignez à aucun rapport.
#
# Usage : scripts/tracer-rdp.sh <hôte> <port> <user> <mdp> [secondes] [options…]
# Les options supplémentaires sont transmises telles quelles au processus RDP
# (par exemple --sans-nla).
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

[ $# -ge 4 ] || { echo "usage : $0 <hôte> <port> <user> <mdp> [secondes]" >&2; exit 2; }
HOTE="$1"; PORT="$2"; USER_="$3"; MDP="$4"; DUREE="${5:-20}"
shift 5 2>/dev/null || shift $#   # ce qui reste part au processus RDP
RDP="rdp-sidecar/target/release/avash-rdp"
[ -x "$RDP" ] || { echo "construisez d'abord le processus RDP" >&2; exit 2; }

TRAVAIL="$(mktemp -d)"; chmod 700 "$TRAVAIL"
nettoyer() { sudo pkill -f "tcpdump -i any -w $TRAVAIL" 2>/dev/null || true; rm -rf "$TRAVAIL"; }
trap nettoyer EXIT

# L'interface « any » couvre la boucle locale comme le réseau.
sudo tcpdump -i any -w "$TRAVAIL/flux.pcap" "tcp port $PORT" >/dev/null 2>&1 &
sleep 2

echo "▸ session de $DUREE s vers $HOTE:$PORT"
SSLKEYLOGFILE="$TRAVAIL/cles.log" timeout "$DUREE" "$RDP" \
  --host "$HOTE" --port "$PORT" -u "$USER_" -p "$MDP" "$@" \
  --width 1024 --height 768 --shot "$TRAVAIL/ecran.png" 2>&1 | sed 's/^/  /' || true

sleep 2
sudo pkill -f "tcpdump -i any -w $TRAVAIL" 2>/dev/null || true
sleep 1
sudo chmod 644 "$TRAVAIL/flux.pcap" 2>/dev/null || true

echo
echo "▸ dialogue déchiffré"
tshark -r "$TRAVAIL/flux.pcap" -o "tls.keylog_file:$TRAVAIL/cles.log" \
  -T fields -e frame.number -e _ws.col.Protocol -e _ws.col.Info 2>/dev/null \
  | awk -F'\t' '$2 != "TCP" { printf "  %-5s %-8s %s\n", $1, $2, $3 }'
