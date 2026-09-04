#!/usr/bin/env bash
# Régénère les captures d'écran du README (docs/captures/) sur la vraie
# application, avec le harnais de la suite bout en bout : bac à sable semé,
# sshd local, et, si un hôte est donné, un bureau Windows pour la vue RDP.
#
# Usage : scripts/captures-readme.sh [hôte RDP]
#   CAPTURES_RDP_UTILISATEUR : compte du bureau (défaut « Administrateur ») ;
#   le mot de passe est lu dans le trousseau (service avash, username
#   rdp:<utilisateur>@<hôte>:3389), jamais passé en argument.
#
# À lancer après un build release : le binaire embarque le front et la
# version affichée dans la barre latérale est celle de ce binaire.
set -euo pipefail
cd "$(dirname "$0")/.."
[ -x target/release/avash-ui ] || {
  echo "target/release/avash-ui absent : ./scripts/release.sh ou cargo build --release -p avash-ui d'abord" >&2
  exit 1
}
export CAPTURES_DOSSIER="$PWD/docs/captures"
mkdir -p "$CAPTURES_DOSSIER"
if [ -n "${1:-}" ]; then
  export CAPTURES_RDP_HOTE="$1"
  export CAPTURES_RDP_UTILISATEUR="${CAPTURES_RDP_UTILISATEUR:-Administrateur}"
  CAPTURES_RDP_MDP="$(secret-tool lookup service avash username "rdp:$CAPTURES_RDP_UTILISATEUR@$1:3389")"
  export CAPTURES_RDP_MDP
  [ -n "$CAPTURES_RDP_MDP" ] || { echo "aucun mot de passe dans le trousseau pour rdp:$CAPTURES_RDP_UTILISATEUR@$1:3389" >&2; exit 1; }
fi
CADRES="$CAPTURES_DOSSIER/cadres"
rm -rf "$CADRES"; mkdir -p "$CADRES"
export CAPTURES_CADRES="$CADRES"
cd e2e
xvfb-run -a -s "-screen 0 1440x900x24" npx wdio run wdio.captures.conf.js
# Les PNG perdent leurs métadonnées (dates, logiciel) : rien d'autre que l'image.
for f in "$CAPTURES_DOSSIER"/*.png; do magick "$f" -strip "$f"; done
# La démonstration : les cadres pris aux moments clés, deux et demi par
# seconde, en WebP animé (que GitHub et GitLab affichent, cinq fois plus
# léger qu'un GIF). Un cadre répété tient une vue à l'écran.
if ls "$CADRES"/*.png >/dev/null 2>&1; then
  ffmpeg -y -hide_banner -loglevel error -framerate 2.5 -pattern_type glob -i "$CADRES/*.png" \
    -vf "scale=960:-1:flags=lanczos" -loop 0 -c:v libwebp_anim -quality 72 -compression_level 6 \
    "$CAPTURES_DOSSIER/demo.webp"
  rm -rf "$CADRES"
fi
ls -la "$CAPTURES_DOSSIER"
