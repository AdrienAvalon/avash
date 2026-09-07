#!/usr/bin/env bash
# Contrôle reproductible du manifeste de mise à jour (latest.json) produit par
# .github/workflows/release.yml.
#
# Trouvé par l'audit du 7 septembre 2026 : tauri-cli 2.11.4 signe aussi les
# paquets .deb et .rpm quand createUpdaterArtifacts est vrai (bundle.rs,
# sign_updaters : PackageType::Deb | PackageType::Rpm), mais l'étape latest.json
# n'émettait que linux-x86_64 (AppImage). Une installation .deb interroge la
# plateforme linux-x86_64-deb, ne la trouvait pas, se rabattait sur l'AppImage
# et échouait après l'avoir téléchargée (« invalid updater format »). Ce test
# extrait le générateur Python réel du workflow et vérifie qu'avec des
# signatures deb/rpm présentes, il émet bien linux-x86_64-deb et
# linux-x86_64-rpm aux URL attendues par le README (Avash_<v>_amd64.deb,
# Avash-<v>-1.x86_64.rpm).
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

WORKFLOW=".github/workflows/release.yml"

python3 - "$WORKFLOW" <<'PY'
import json, subprocess, sys, yaml

workflow = yaml.safe_load(open(sys.argv[1], encoding="utf-8"))
etapes = workflow["jobs"]["publier"]["steps"]
run = next(
    e["run"] for e in etapes
    if e.get("name", "").startswith("Manifeste de mise à jour")
)

# Le script YAML est déjà désindenté : on isole le programme Python du heredoc
# `<<'PY' ... PY` (la ligne d'ouverture porte encore « > latest.json » après le
# marqueur) et on le rejoue comme le ferait le job, avec des signatures deb/rpm
# factices présentes.
apres_ouverture = run.split("<<'PY'", 1)[1].split("\n", 1)[1]
corps = apres_ouverture.split("\nPY", 1)[0]

version = "0.9.2"
base = "https://example.invalid/download/v0.9.2"
# Ordre des arguments attendu par le programme : sig AppImage, sig setup.exe,
# sig .app.tar.gz, sig .deb, sig .rpm.
args = [version, base, "sig-appimage", "", "", "sig-deb", "sig-rpm"]

sortie = subprocess.run(
    [sys.executable, "-c", corps, *args],
    capture_output=True, text=True,
)
if sortie.returncode != 0:
    print("Le générateur latest.json a échoué :", file=sys.stderr)
    print(sortie.stderr, file=sys.stderr)
    sys.exit(1)

manifeste = json.loads(sortie.stdout)
plateformes = manifeste.get("platforms", {})

echecs = []
if "linux-x86_64-deb" not in plateformes:
    echecs.append(
        "plateforme linux-x86_64-deb absente : une installation .deb se rabat "
        "sur l'AppImage et échoue à l'installer"
    )
else:
    url = plateformes["linux-x86_64-deb"].get("url", "")
    if not url.endswith(f"Avash_{version}_amd64.deb"):
        echecs.append(f"URL deb inattendue : {url}")

if "linux-x86_64-rpm" not in plateformes:
    echecs.append(
        "plateforme linux-x86_64-rpm absente : une installation .rpm se rabat "
        "sur l'AppImage et échoue à l'installer"
    )
else:
    url = plateformes["linux-x86_64-rpm"].get("url", "")
    if not url.endswith(f"Avash-{version}-1.x86_64.rpm"):
        echecs.append(f"URL rpm inattendue : {url}")

if echecs:
    for e in echecs:
        print("  ✗", e, file=sys.stderr)
    sys.exit(1)

print("  ✓ latest.json émet linux-x86_64-deb et linux-x86_64-rpm aux bonnes URL")
PY
