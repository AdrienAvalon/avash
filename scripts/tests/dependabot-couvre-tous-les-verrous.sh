#!/usr/bin/env bash
# Contrôle reproductible : chaque fichier de verrouillage suivi par git est soit
# surveillé par Dependabot, soit exclu ici avec sa raison ; et le package.json
# de crates/avash-ui reste un simple marqueur, sans dépendance ni verrou.
#
# Trouvé par l'audit du 12 septembre 2026 (C-chaine-8) : crates/avash-ui
# portait un package.json et un package-lock.json (@tauri-apps/cli ^2) que rien
# n'installait, n'auditait ni ne mettait à jour, et qui bruitaient le SBOM et
# les audits. Le package.json, lui, ne peut pas partir : tauri-cli prend pour
# dossier du front celui qui contient un package.json (app_paths.rs,
# resolve_frontend_dir) et y lance beforeBuildCommand (`npm --prefix
# ../../web`). Sans lui, il se rabat sur crates/ et le build casse. Il reste
# donc, vidé de toute dépendance ; tauri-cli s'installe par cargo, en version
# exacte (ci-outils-epingles.sh).
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import json
import os
import subprocess
import sys

import yaml

# Verrous délibérément hors de Dependabot, et pourquoi.
EXCLUS = {
    "fuzz": "cibles cargo-fuzz (nightly), jamais livrées ; suivent l'espace de travail par chemin",
    "test-rdp-server": "serveur de test de la suite bout en bout, jamais livré",
    "test-vnc-server": "serveur de test de la suite bout en bout, jamais livré",
    "test-rdp-server/vendor/ironrdp-server": "paquet porté du serveur de test, suivi par portes-amont.sh",
}
PORTES = "rdp-sidecar/vendor/"  # paquets portés : suivis par scripts/portes-amont.sh (qualite.yml)

echecs = []
dependabot = yaml.safe_load(open(".github/dependabot.yml", encoding="utf-8"))
couverts = {(u["package-ecosystem"], u["directory"].strip("/")) for u in dependabot["updates"]}

suivis = subprocess.run(["git", "ls-files"], capture_output=True, text=True, check=True).stdout.split()
for f in suivis:
    if not os.path.exists(f):
        continue  # supprimé de l'arbre, pas encore de l'index
    base = os.path.basename(f)
    if base == "Cargo.lock":
        eco = "cargo"
    elif base == "package-lock.json":
        eco = "npm"
    else:
        continue
    dossier = os.path.dirname(f)
    if (eco, dossier) in couverts or dossier in EXCLUS or dossier.startswith(PORTES):
        continue
    echecs.append(f"{f} : ni surveillé par Dependabot ({eco}, /{dossier}) ni exclu avec une raison")

marqueur = "crates/avash-ui/package.json"
if os.path.exists(marqueur):
    contenu = json.load(open(marqueur, encoding="utf-8"))
    for cle in ("dependencies", "devDependencies", "optionalDependencies", "peerDependencies"):
        if contenu.get(cle):
            echecs.append(f"{marqueur} : `{cle}` non vide, alors que rien n'installe ce dossier")
else:
    echecs.append(f"{marqueur} absent : tauri-cli lancerait beforeBuildCommand depuis crates/ (../../web introuvable)")
if os.path.exists("crates/avash-ui/package-lock.json"):
    echecs.append("crates/avash-ui/package-lock.json : verrou que rien n'installe, n'audite ni ne met à jour")

if echecs:
    for e in echecs:
        print(f"  ✗ {e}", file=sys.stderr)
    sys.exit(1)
print("  ✓ chaque verrou suivi est surveillé par Dependabot ou exclu avec sa raison ; avash-ui n'a qu'un marqueur")
PY
