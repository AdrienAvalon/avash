#!/usr/bin/env bash
# Contrôle reproductible : docs/architecture.md reste accordé au code sur trois
# points qui dérivent en silence quand on ajoute une cible, un protocole ou un
# add-on sans rouvrir ce document.
#
# Trouvé par l'audit du 8 septembre 2026 : trois écarts entre le doc et le code.
#   1. La puce « Distribution » (l.382) n'annonçait que « AppImage sous Linux,
#      installeur NSIS sous Windows », alors que tauri.conf.json construit aussi
#      deb et rpm, et release.yml publie une archive portable Windows et un
#      .dmg macOS : un contributeur lisant l'architecture ignorait ces canaux.
#   2. La phrase d'ouverture (l.3) présentait « SSH et RDP », VNC et le port
#      série manquant alors que le même document les décrit (VNC, série).
#   3. La liste des add-ons xterm (l.131) omettait `serialize`, pourtant
#      déclaré par web/package.json et chargé par web/xterm-charge.ts.
#
# Les trois vérités vivent dans le code : ce contrôle les y lit et exige que le
# doc les reprenne. Il rougit contre l'ancien état.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import json
import pathlib
import re
import sys

echecs = []

archi = pathlib.Path("docs/architecture.md").read_text(encoding="utf-8")
lignes = archi.splitlines()

# 1. Distribution : les cibles du bundler et les artefacts publiés.
targets = set(json.loads(pathlib.Path("crates/avash-ui/tauri.conf.json").read_text(
    encoding="utf-8"))["bundle"]["targets"])
release = pathlib.Path(".github/workflows/release.yml").read_text(encoding="utf-8")
# On repère la puce « Distribution » et le reste de la même puce (jusqu'à la
# prochaine puce ou le prochain titre).
distrib = None
for i, ligne in enumerate(lignes):
    if re.search(r"\*\*Distribution\*\*", ligne):
        bloc = [ligne]
        for suite in lignes[i + 1:]:
            if re.match(r"^\s*-\s", suite) or suite.startswith("#"):
                break
            bloc.append(suite)
        distrib = " ".join(bloc)
        break
if distrib is None:
    echecs.append("docs/architecture.md : puce « **Distribution** » introuvable")
else:
    bas = distrib.lower()
    # Chaque paquet Linux supplémentaire que le bundler produit doit être cité.
    for cible, mot in (("deb", "deb"), ("rpm", "rpm")):
        if cible in targets and mot not in bas:
            echecs.append(
                f"docs/architecture.md : la puce Distribution ne cite pas le "
                f"paquet `.{mot}` (tauri.conf.json targets contient « {cible} »)")
    # release.yml publie un .dmg macOS : la puce doit mentionner macOS.
    if "*.dmg" in release and not re.search(r"macos|mac\s?os|\.dmg|image disque", bas):
        echecs.append(
            "docs/architecture.md : la puce Distribution ne mentionne pas macOS "
            "(release.yml publie un artefact `*.dmg`)")
    # release.yml publie une archive portable Windows.
    if "portable/*.zip" in release and "portable" not in bas:
        echecs.append(
            "docs/architecture.md : la puce Distribution ne mentionne pas "
            "l'archive portable (release.yml publie `portable/*.zip`)")

# 2. Périmètre : la phrase d'ouverture doit nommer les protocoles que le
# document lui-même décrit. VNC a sa section (« bureaux **VNC** »), le port
# série la sienne (« **port série** »/`serie.rs`).
intro = next((l for l in lignes if l.startswith("Avash est un gestionnaire")), "")
decrit_vnc = "**VNC**" in archi
decrit_serie = "port série" in archi
if decrit_vnc and "VNC" not in intro:
    echecs.append(
        "docs/architecture.md : la phrase d'ouverture omet VNC, pourtant décrit "
        "dans le document")
if decrit_serie and "série" not in intro:
    echecs.append(
        "docs/architecture.md : la phrase d'ouverture omet le port série, "
        "pourtant décrit dans le document")

# 3. Add-ons xterm : chaque add-on déclaré dans web/package.json et chargé par
# xterm-charge.ts doit figurer dans la liste du doc.
pkg = json.loads(pathlib.Path("web/package.json").read_text(encoding="utf-8"))
deps = {**pkg.get("dependencies", {}), **pkg.get("devDependencies", {})}
addons = {m.group(1) for d in deps
          if (m := re.fullmatch(r"@xterm/addon-(.+)", d))}
# La liste vit dans le paragraphe contenant « add-ons » (peut tenir sur deux
# lignes) : on rassemble ce paragraphe.
para_addons = None
for i, ligne in enumerate(lignes):
    if "add-ons" in ligne:
        bloc = [ligne]
        for suite in lignes[i + 1:]:
            if not suite.strip():
                break
            bloc.append(suite)
        para_addons = " ".join(bloc)
        break
if para_addons is None:
    echecs.append("docs/architecture.md : liste des add-ons xterm introuvable")
else:
    for addon in sorted(addons):
        if f"`{addon}`" not in para_addons:
            echecs.append(
                f"docs/architecture.md : la liste des add-ons xterm omet "
                f"`{addon}` (déclaré `@xterm/addon-{addon}` dans web/package.json)")

if echecs:
    for e in echecs:
        print(f"  ✗ {e}", file=sys.stderr)
    print("  → mettre docs/architecture.md à jour d'après le code", file=sys.stderr)
    sys.exit(1)

print("  ✓ docs/architecture.md : distribution, périmètre et add-ons accordés au code")
PY
