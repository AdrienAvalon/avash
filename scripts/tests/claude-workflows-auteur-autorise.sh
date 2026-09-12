#!/usr/bin/env bash
# Contrôle reproductible : les workflows Claude ne démarrent que pour un auteur
# qui a déjà des droits sur le dépôt, et la revue automatique ignore les PR
# venues d'un fork.
#
# Trouvé par l'audit du 12 septembre 2026 (C-chaine-6) : la condition de
# claude.yml ne regardait que la présence de « @claude ». N'importe quel
# inconnu démarrait le job, que l'action refusait ensuite (minutes perdues), et
# rien dans le workflow lui-même ne disait qui avait le droit de faire agir
# Claude, qui pousse avec le jeton de l'App GitHub. Chaque branche du `if:`
# exige maintenant `author_association` parmi OWNER, MEMBER, COLLABORATOR ; une
# PR de fork n'a de toute façon pas accès au secret, on ne la lance plus.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import sys

import yaml

echecs = []
ROLES = ("OWNER", "MEMBER", "COLLABORATOR")

wf = yaml.safe_load(open(".github/workflows/claude.yml", encoding="utf-8"))
# PyYAML lit la clé `on` comme le booléen True.
evenements = list((wf.get("on") or wf.get(True) or {}).keys())
job = wf["jobs"]["claude"]
condition = str(job.get("if", ""))
for ev in evenements:
    clauses = [l for l in condition.splitlines() if f"github.event_name == '{ev}'" in l]
    if not clauses:
        echecs.append(f"claude.yml : aucune clause pour l'événement {ev}")
        continue
    for c in clauses:
        if "author_association" not in c or not all(r in c for r in ROLES):
            echecs.append(f"claude.yml : la clause {ev} ne restreint pas l'auteur à {', '.join(ROLES)}")

revue = yaml.safe_load(open(".github/workflows/claude-code-review.yml", encoding="utf-8"))
cond_revue = str(revue["jobs"]["claude-review"].get("if", ""))
if "github.event.pull_request.head.repo.full_name == github.repository" not in cond_revue:
    echecs.append("claude-code-review.yml : les PR venues d'un fork ne sont pas exclues")
if "dependabot[bot]" not in cond_revue:
    echecs.append("claude-code-review.yml : l'exclusion des PR Dependabot a disparu")

if echecs:
    for e in echecs:
        print(f"  ✗ {e}", file=sys.stderr)
    sys.exit(1)
print(f"  ✓ claude.yml restreint {len(evenements)} événements aux auteurs du dépôt ; la revue ignore forks et Dependabot")
PY
