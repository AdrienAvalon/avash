#!/usr/bin/env bash
# Contrôle reproductible du job `fuzz` de .github/workflows/securite.yml.
#
# Trouvé par l'audit du 7 septembre 2026 : le job `fuzz` portait
# `if: github.event_name != 'pull_request'`, donc il était sauté sur les PR.
# Or `check.sh` ne compile pas le crate `fuzz` (hors espace de travail, nightly
# seulement) : une PR qui renommait un parseur du cœur ou changeait une
# signature utilisée par une cible (p. ex. `avash::import::parse_reg_query`,
# `ClearCodecDecoder::decode`) passait verte partout, puis ne cassait le job
# `fuzz` qu'au premier push sur `main` après la fusion. La rupture d'API
# n'était vue qu'une fois déjà fusionnée.
#
# Ce test rejoue la sélection d'événement de GitHub (`github.event_name ==
# 'pull_request'`) sur le job `fuzz` du workflow réel et exige que, sur une PR :
#   - le job ne soit pas sauté (pas de `if` de job qui exclut pull_request) ;
#   - une étape effective lance `cargo … fuzz build` (compilation des cibles).
# Contre le workflow d'origine (job sauté sur PR) il échoue ; avec le job qui
# compile les cibles sur PR il passe. On n'exige PAS de fuzzer sur PR (coûteux,
# non déterministe) : seulement de compiler, ce qui suffit à voir la rupture.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

WORKFLOW=".github/workflows/securite.yml"

python3 - "$WORKFLOW" <<'PY'
import re, sys, yaml

workflow = yaml.safe_load(open(sys.argv[1], encoding="utf-8"))
job = workflow["jobs"]["fuzz"]


def s_actif(expr):
    """Évalue une condition `if` de workflow pour l'événement pull_request.

    On ne modélise que les formes réellement écrites dans ce workflow :
    absente (toujours vraie), une comparaison sur `github.event_name`, et les
    fonctions d'état `failure()`/`success()`/`always()`. Toute autre forme est
    signalée plutôt que devinée à tort.
    """
    if expr is None:
        return True
    e = str(expr).strip()
    # `${{ … }}` est optionnel autour d'une condition ; on le retire.
    m = re.fullmatch(r"\$\{\{(.*)\}\}", e, re.S)
    if m:
        e = m.group(1).strip()
    if e in ("success()", "always()"):
        return True
    if e == "failure()":
        return False  # pas d'échec quand on évalue le déroulement nominal
    m = re.fullmatch(r"github\.event_name\s*(==|!=)\s*'pull_request'", e)
    if m:
        return m.group(1) == "=="
    print(f"  ~ condition `if` non modélisée : {e!r}, contrôle sauté", file=sys.stderr)
    sys.exit(0)


# 1) Sur une PR, le job doit tourner.
if not s_actif(job.get("if")):
    print(
        "  ✗ le job `fuzz` est sauté sur les pull_request "
        f"(if: {job.get('if')!r}) : une rupture d'API dans une cible de fuzzing "
        "n'est vue qu'après la fusion, au premier push sur main",
        file=sys.stderr,
    )
    sys.exit(1)

# 2) Sur une PR, une étape effective doit compiler les cibles (`cargo fuzz build`).
build = re.compile(r"\bcargo\b.*\bfuzz\b.*\bbuild\b")
compile_sur_pr = any(
    s_actif(etape.get("if")) and build.search(etape.get("run", ""))
    for etape in job["steps"]
)
if not compile_sur_pr:
    print(
        "  ✗ le job `fuzz` ne compile aucune cible sur pull_request : "
        "un renommage ou une signature changée d'un parseur du cœur passe "
        "verte et ne casse le job `fuzz` qu'après la fusion sur main",
        file=sys.stderr,
    )
    sys.exit(1)

print("  ✓ le job `fuzz` compile les cibles sur pull_request (rupture d'API vue avant fusion)")
PY
