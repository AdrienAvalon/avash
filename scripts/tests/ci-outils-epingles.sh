#!/usr/bin/env bash
# Contrôle reproductible : les outils installés par la chaîne le sont en
# version exacte, et chaque outil a la même version partout.
#
# Trouvé par l'audit du 12 septembre 2026 (C-chaine-5) : `cargo install
# tauri-cli --version "^2" --locked` figeait les dépendances de l'outil, pas sa
# version, et tauri-cli est l'outil qui assemble ET signe les bundles. Une
# version défectueuse ou malveillante publiée la veille servait telle quelle à
# une release signée, et deux exécutions du même tag ne produisaient pas les
# mêmes binaires. Même flottement pour cargo-audit, cargo-deny, tauri-driver,
# cargo-fuzz, cargo-llvm-cov et cargo-mutants ; Dependabot ne voit aucune de
# ces lignes, d'où l'exigence de cohérence : une montée se fait partout à la
# fois, à la main.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import collections
import glob
import re
import sys

import yaml

EXACTE = re.compile(r"^=?\d+\.\d+\.\d+$")
AVEC_VALEUR = {"--version", "--vers", "--root", "--target", "--features", "-F", "--profile",
               "--index", "--registry", "--git", "--branch", "--tag", "--rev", "--path",
               "-j", "--jobs", "--target-dir", "--config", "-Z"}
fichiers = sorted(
    glob.glob(".github/workflows/*.yml")
    + [".gitlab-ci.yml", "ci/Dockerfile", "check.sh", "scripts/hooks/pre-commit"]
    + glob.glob("scripts/*.sh")
)
echecs = []
versions = collections.defaultdict(set)


def lignes_logiques(texte):
    tampon, debut = "", None
    for n, ligne in enumerate(texte.splitlines(), 1):
        if debut is None:
            debut = n
        if ligne.rstrip().endswith("\\"):
            tampon += ligne.rstrip()[:-1] + " "
            continue
        yield debut, tampon + ligne
        tampon, debut = "", None


for chemin in fichiers:
    try:
        texte = open(chemin, encoding="utf-8").read()
    except FileNotFoundError:
        continue
    for n, ligne in lignes_logiques(texte):
        if ligne.strip().startswith("#"):
            continue
        for m in re.finditer(r"\bcargo\s+install\b(.*)", ligne):
            reste = re.split(r"\s(?:&&|\|\||;)\s|\"|'\s*[>)]|\s>&2", m.group(1))[0]
            jetons = reste.replace("'", " ").replace('"', " ").split()
            crates, version, i = [], None, 0
            while i < len(jetons):
                j = jetons[i]
                if j in ("--version", "--vers"):
                    version = jetons[i + 1] if i + 1 < len(jetons) else ""
                    i += 2
                    continue
                if j in AVEC_VALEUR:
                    i += 2
                    continue
                if j.startswith("-"):
                    i += 1
                    continue
                crates.append(j)
                i += 1
            lieu = f"{chemin}:{n}"
            if "--locked" not in jetons:
                echecs.append(f"{lieu} : cargo install sans --locked")
            if not crates:
                echecs.append(f"{lieu} : aucun paquet lisible dans « {reste.strip()} »")
            for c in crates:
                if "@" in c:
                    nom, v = c.split("@", 1)
                elif version is not None and len(crates) == 1:
                    nom, v = c, version
                else:
                    nom, v = c, None
                if v is None or not EXACTE.match(v):
                    echecs.append(f"{lieu} : {nom} sans version exacte ({v or 'aucune'})")
                else:
                    versions[nom].add((v.lstrip("="), lieu))

for chemin in sorted(glob.glob(".github/workflows/*.yml")):
    wf = yaml.safe_load(open(chemin, encoding="utf-8"))
    for nom_job, job in (wf.get("jobs") or {}).items():
        for etape in job.get("steps") or []:
            if not str(etape.get("uses", "")).startswith("taiki-e/install-action@"):
                continue
            outils = str((etape.get("with") or {}).get("tool", ""))
            for outil in filter(None, (o.strip() for o in outils.split(","))):
                lieu = f"{chemin}:{nom_job}"
                if "@" not in outil or not EXACTE.match(outil.split("@", 1)[1]):
                    echecs.append(f"{lieu} : install-action installe {outil} sans version exacte")
                else:
                    nom, v = outil.split("@", 1)
                    versions[nom].add((v, lieu))

for nom, occurrences in sorted(versions.items()):
    distinctes = {v for v, _ in occurrences}
    if len(distinctes) > 1:
        detail = ", ".join(f"{v} ({lieu})" for v, lieu in sorted(occurrences))
        echecs.append(f"{nom} : versions divergentes : {detail}")

if echecs:
    for e in echecs:
        print(f"  ✗ {e}", file=sys.stderr)
    sys.exit(1)
resume = ", ".join(f"{nom} {next(iter({v for v, _ in occ}))}" for nom, occ in sorted(versions.items()))
print(f"  ✓ outils épinglés et cohérents : {resume}")
PY
