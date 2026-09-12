#!/usr/bin/env bash
# Contrôle reproductible : aucun `actions/checkout` ne laisse le GITHUB_TOKEN
# dans .git/config, sauf dans un job qui pousse lui-même avec git.
#
# Trouvé par l'audit du 12 septembre 2026 (C-chaine-7) : seul le Scorecard
# posait `persist-credentials: false`. Dans `publier` (release.yml, droits
# contents: write), le jeton capable de réécrire les fichiers de release et de
# pousser des tags restait lisible par les quatre actions tierces qui suivent.
# Aucun job ne pousse avec git : claude-code-action retire l'en-tête posé par
# checkout et installe sa propre authentification (git-config.ts, commit
# épinglé 9c5ddab), il n'a pas besoin du jeton persistant.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import glob
import re
import sys

import yaml

echecs, vus = [], 0
for chemin in sorted(glob.glob(".github/workflows/*.yml")):
    wf = yaml.safe_load(open(chemin, encoding="utf-8"))
    for nom_job, job in (wf.get("jobs") or {}).items():
        etapes = job.get("steps") or []
        pousse = any(re.search(r"\bgit\s+push\b", str(e.get("run", ""))) for e in etapes)
        for etape in etapes:
            if not str(etape.get("uses", "")).startswith("actions/checkout@"):
                continue
            vus += 1
            persiste = (etape.get("with") or {}).get("persist-credentials", True)
            if persiste is not False and not pousse:
                echecs.append(f"{chemin.split('/')[-1]}:{nom_job} : checkout sans `persist-credentials: false`")

if echecs:
    for e in echecs:
        print(f"  ✗ {e}", file=sys.stderr)
    sys.exit(1)
print(f"  ✓ {vus} checkouts, aucun ne laisse le jeton dans .git/config")
PY
