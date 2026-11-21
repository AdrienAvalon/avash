#!/usr/bin/env bash
# Régénère docs/captures/apercu-social.png, l'image que montrent les réseaux et
# les messageries quand on y colle l'adresse de la vitrine (`og:image`).
#
# L'ancienne était un PNG sans source ni script : au changement de logo du
# 21 novembre 2026, il a fallu la redessiner. Elle est désormais décrite ici,
# en SVG, avec la marque de icons/logo.svg, et rendue par rsvg-convert.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command -v rsvg-convert >/dev/null || { echo "✗ apercu-social : rsvg-convert introuvable" >&2; exit 1; }

tmp="$(mktemp --suffix=.svg)"
trap 'rm -f -- "$tmp"' EXIT
cat > "$tmp" <<'SVG'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1280 640" width="1280" height="640">
  <rect width="1280" height="640" fill="#0c0f14"/>
  <g transform="translate(140 170)">
    <rect width="300" height="300" rx="68" fill="#10141b" stroke="#283040" stroke-width="3"/>
    <polyline points="76,226 150,68 224,226" fill="none" stroke="#eef0f4" stroke-width="30" stroke-linecap="round" stroke-linejoin="round"/>
    <rect x="128" y="170" width="44" height="36" rx="8" fill="#4ade9f"/>
  </g>
  <g font-family="'Noto Sans', 'DejaVu Sans', sans-serif">
    <text x="540" y="258" font-size="118" font-weight="700" fill="#ffffff">avash</text>
    <text x="544" y="322" font-size="34" fill="#c9cfda">Gestionnaire de connexions SSH et RDP</text>
    <text x="544" y="368" font-size="34" fill="#c9cfda">natif, rapide, sécurisé</text>
    <text x="544" y="442" font-size="25" fill="#4ade9f">Tauri 2 + Rust  ·  Linux · Windows · macOS  ·  AGPL-3.0</text>
    <text x="544" y="488" font-size="22" fill="#8690a1">SSH · RDP · VNC · SFTP · tunnels · import PuTTY et MobaXterm</text>
  </g>
</svg>
SVG
rsvg-convert -w 1280 -h 640 "$tmp" -o docs/captures/apercu-social.png
echo "✓ apercu-social : docs/captures/apercu-social.png régénéré"
