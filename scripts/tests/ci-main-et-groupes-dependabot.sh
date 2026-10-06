#!/usr/bin/env bash
# Contrôle reproductible : deux réglages de la chaîne nés du 6 octobre 2026.
#
# 1. CI et Sécurité n'annulent jamais une exécution sur main. Trois poussées
#    rapprochées (fusion de #58, publication 0.13.2, canaux) avaient annulé les
#    exécutions de main l'une après l'autre ; GitHub compte une exécution
#    annulée comme un échec, et le badge CI du README a affiché « failing »
#    sans qu'aucun test ait échoué. Les PR gardent l'annulation.
# 2. Dependabot propose ensemble ce qui doit bouger ensemble : les trois étapes
#    de codeql-action (proposées séparément, #37, #39 et #40, init et analyze
#    désaccordées faisaient échouer CodeQL) et les paquets @wdio/* de la suite
#    bout en bout (#46 à #49 se mettaient en conflit sur e2e/package-lock.json).
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - "${1:-.}" <<'PY'
import sys
import yaml

racine = sys.argv[1]
echecs = []

for wf in ("ci.yml", "securite.yml"):
    with open(f"{racine}/.github/workflows/{wf}", encoding="utf-8") as f:
        c = (yaml.safe_load(f) or {}).get("concurrency") or {}
    v = str(c.get("cancel-in-progress", "")).replace(" ", "")
    if v != "${{github.ref!='refs/heads/main'}}":
        echecs.append(f"{wf} : cancel-in-progress vaut « {c.get('cancel-in-progress')} », "
                      "attendu ${{ github.ref != 'refs/heads/main' }}")

with open(f"{racine}/.github/dependabot.yml", encoding="utf-8") as f:
    maj = (yaml.safe_load(f) or {}).get("updates") or []


def motifs(ecosysteme, dossier):
    for u in maj:
        if u.get("package-ecosystem") == ecosysteme and u.get("directory") == dossier:
            return {m for g in (u.get("groups") or {}).values() for m in g.get("patterns", [])}
    return set()


if "github/codeql-action/*" not in motifs("github-actions", "/"):
    echecs.append("dependabot.yml : aucun groupe github/codeql-action/* pour les actions")
if "@wdio/*" not in motifs("npm", "/e2e"):
    echecs.append("dependabot.yml : aucun groupe @wdio/* pour e2e/")

for e in echecs:
    print(f"  ✗ {e}", file=sys.stderr)
if echecs:
    sys.exit(1)
print("  ✓ main jamais annulée (CI, Sécurité) ; codeql-action et @wdio/* groupés par Dependabot")
PY
