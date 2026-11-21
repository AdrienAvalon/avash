#!/usr/bin/env bash
# Régénère le jeu d'icônes de l'application depuis sa source unique,
# crates/avash-ui/icons/logo.svg.
#
# Jusqu'au changement de logo du 21 novembre 2026, rien ne disait comment ces
# dix-sept fichiers avaient été produits : ils dataient tous de la même minute
# du 30 août, sans script. Les retoucher un par un aurait laissé des tailles
# sur l'ancien dessin. Ce script les refait tous, à l'identique d'un passage à
# l'autre (mêmes outils, mêmes tailles), et copie la marque là où le dépôt la
# sert ailleurs (accueil du front, README, vitrine).
#
# Prérequis : rsvg-convert (librsvg) et magick (ImageMagick 7).
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

for outil in rsvg-convert magick; do
  command -v "$outil" >/dev/null || { echo "✗ icones : $outil introuvable" >&2; exit 1; }
done

icones="crates/avash-ui/icons"
source="$icones/logo.svg"

rendre() { rsvg-convert -w "$1" -h "$1" "$source" -o "$2"; }

# Linux et le manifeste Tauri.
rendre 32 "$icones/32x32.png"
rendre 64 "$icones/64x64.png"
rendre 128 "$icones/128x128.png"
rendre 256 "$icones/128x128@2x.png"
rendre 512 "$icones/icon.png"

# Tuiles Windows (paquet Appx) et logo du Store.
for t in 30 44 71 89 107 142 150 284 310; do
  rendre "$t" "$icones/Square${t}x${t}Logo.png"
done
rendre 50 "$icones/StoreLogo.png"

# icon.ico : chaque taille est rendue depuis le SVG, pas réduite depuis la plus
# grande, pour que le trait reste net à 16 px. Même ordre que l'ancien fichier
# (32 en tête : c'est l'entrée que lit l'explorateur de Windows par défaut).
tmp="$(mktemp -d)"
trap 'rm -rf -- "$tmp"' EXIT
for t in 32 16 24 48 64 256; do rendre "$t" "$tmp/$t.png"; done
magick "$tmp/32.png" "$tmp/16.png" "$tmp/24.png" "$tmp/48.png" "$tmp/64.png" "$tmp/256.png" "$icones/icon.ico"

# La même marque pour le README et la vitrine, qui la servent en SVG.
cp "$source" web/public/hero.svg
cp "$source" site/hero.svg

echo "✓ icones : jeu régénéré depuis $source"
