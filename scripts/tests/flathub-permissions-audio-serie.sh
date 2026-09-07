#!/usr/bin/env bash
# Contrôle reproductible des permissions du manifeste Flathub touchant deux
# fonctions du produit : le son du bureau distant et les consoles série.
#
# Trouvé par l'audit du 7 septembre 2026 : les finish-args n'accordaient que
# `--device=dri` (le seul /dev/dri) et aucun socket audio. Or le son RDP est
# joué par la webview (web/audio.ts, `new AudioContext`), donc par
# WebKitGTK/GStreamer, qui a besoin du serveur audio de l'hôte : sans
# `--socket=pulseaudio` (PipeWire compris) la fonction « son du bureau » est
# muette dans le bac à sable. Les consoles série (crates/avash/src/serie.rs,
# `serialport::available_ports()` sur /dev/ttyUSB*, /dev/ttyACM*) ne voient
# aucun périphérique tant que `--device=all` n'expose pas ces nœuds : il
# n'existe pas de `--device=` plus étroit pour les tty. Ce test lit le
# manifeste et exige les deux droits ; il vérifie aussi que RELEASE.md §8 les
# liste parmi les exceptions à justifier dans la PR de soumission.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

MANIFESTE="packaging/flathub/io.github.AdrienAvalon.avash.yml"
RELEASE="RELEASE.md"

python3 - "$MANIFESTE" "$RELEASE" <<'PY'
import sys, yaml

manifeste = yaml.safe_load(open(sys.argv[1], encoding="utf-8"))
finish = manifeste.get("finish-args", [])
release = open(sys.argv[2], encoding="utf-8").read()

echecs = []

# Son RDP : la webview (WebKitGTK/GStreamer) a besoin du serveur audio de l'hôte.
if "--socket=pulseaudio" not in finish:
    echecs.append(
        "finish-args sans --socket=pulseaudio : le son du bureau distant "
        "(web/audio.ts via WebKitGTK/GStreamer) est muet dans le bac à sable"
    )

# Consoles série : /dev/ttyUSB*, /dev/ttyACM* ne sont exposés que par
# --device=all (aucune option --device= ne cible finement les tty).
if "--device=all" not in finish:
    echecs.append(
        "finish-args sans --device=all : les ports série "
        "(crates/avash/src/serie.rs sur /dev/ttyUSB*, /dev/ttyACM*) "
        "restent invisibles dans le bac à sable"
    )

# Les deux droits, larges, doivent figurer dans les exceptions à justifier
# (RELEASE.md §8), sinon la PR de soumission les oublie et le robot Flathub
# bloque.
for droit in ("--socket=pulseaudio", "--device=all"):
    if droit not in release:
        echecs.append(f"{droit} absent des exceptions à justifier de RELEASE.md §8")

if echecs:
    for e in echecs:
        print("  ✗", e, file=sys.stderr)
    sys.exit(1)

print("  ✓ le manifeste Flathub accorde le son (pulseaudio) et le série (device=all), justifiés dans RELEASE.md")
PY
