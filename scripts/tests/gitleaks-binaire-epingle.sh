#!/usr/bin/env bash
# Contrôle reproductible : chaque étape gitleaks-action fixe la version du
# binaire qu'elle télécharge, la même partout.
#
# Trouvé par l'audit du 12 septembre 2026 (C-chaine-13) : l'action est épinglée
# sur son commit, mais le binaire qu'elle télécharge à chaque exécution
# dépendait d'une valeur codée dans l'action (8.24.3 au commit e0c47f4), que
# rien dans le dépôt ne montrait et qu'une montée de l'action changeait sans le
# dire. La porte de publication (release.yml, job securite) repose sur ce
# binaire. GITLEAKS_VERSION rend la version visible et relue ; l'action ne
# vérifie toujours aucune empreinte du binaire, résidu écrit dans le rapport.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import glob
import re
import sys

import yaml

echecs, versions = [], {}
for chemin in sorted(glob.glob(".github/workflows/*.yml")):
    wf = yaml.safe_load(open(chemin, encoding="utf-8"))
    for nom_job, job in (wf.get("jobs") or {}).items():
        for etape in job.get("steps") or []:
            if not str(etape.get("uses", "")).startswith("gitleaks/gitleaks-action@"):
                continue
            lieu = f"{chemin.split('/')[-1]}:{nom_job}"
            v = str((etape.get("env") or {}).get("GITLEAKS_VERSION", ""))
            if not re.fullmatch(r"\d+\.\d+\.\d+", v):
                echecs.append(f"{lieu} : GITLEAKS_VERSION absent ou non exact ({v or 'aucun'})")
            else:
                versions[lieu] = v
if not versions and not echecs:
    echecs.append("aucune étape gitleaks-action trouvée")
if len(set(versions.values())) > 1:
    echecs.append("versions de gitleaks divergentes : " + ", ".join(f"{v} ({l})" for l, v in versions.items()))

if echecs:
    for e in echecs:
        print(f"  ✗ {e}", file=sys.stderr)
    sys.exit(1)
print(f"  ✓ gitleaks {next(iter(versions.values()))} fixé sur {len(versions)} étape(s)")
PY
