#!/usr/bin/env bash
# Contrôle reproductible : les jobs qui consomment les secrets de signature et
# celui qui publie passent par l'environnement GitHub `release`.
#
# Trouvé par l'audit du 12 septembre 2026 (C-chaine-3) : les deux secrets Tauri
# vivaient au niveau du dépôt, et aucun humain n'avait à approuver avant que la
# clé serve et que `publier` réécrive latest.json sous tous les utilisateurs.
# Un tag `v*` poussé par un jeton qui fuit, ou par un agent trompé, suffisait à
# produire une release signée. Le relecteur obligatoire se règle sur GitHub
# (voir le rapport de l'audit) ; côté dépôt, il faut que les jobs y soient
# rattachés, sans quoi la protection de l'environnement ne s'applique pas.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import glob
import sys

import yaml

SECRETS = ("secrets.TAURI_SIGNING_PRIVATE_KEY",)
echecs = []
proteges = []


def environnement(job):
    env = job.get("environment")
    if isinstance(env, dict):
        return env.get("name")
    return env


for chemin in sorted(glob.glob(".github/workflows/*.yml")):
    nom_wf = chemin.split("/")[-1]
    wf = yaml.safe_load(open(chemin, encoding="utf-8"))
    for nom_job, job in (wf.get("jobs") or {}).items():
        texte = yaml.safe_dump(job, allow_unicode=True)
        consomme = any(s in texte for s in SECRETS)
        if nom_wf == "release.yml" and nom_job == "publier":
            consomme = True
        if not consomme:
            continue
        court = f"{nom_wf}:{nom_job}"
        if environnement(job) != "release":
            echecs.append(f"{court} : consomme la signature ou publie sans `environment: release`")
        else:
            proteges.append(court)

if "release.yml:publier" not in proteges and not any("publier" in e for e in echecs):
    echecs.append("release.yml : job `publier` introuvable")

if echecs:
    for e in echecs:
        print(f"  ✗ {e}", file=sys.stderr)
    sys.exit(1)
print(f"  ✓ environnement `release` posé sur {', '.join(proteges)}")
PY
