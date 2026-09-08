#!/usr/bin/env bash
# Contrôle reproductible : rust-cache met en cache les répertoires target hors
# espace de travail que chaque job construit réellement.
#
# Trouvé par l'audit du 8 septembre 2026 : Swatinem/rust-cache ne met en cache
# par défaut que `./target` (README : « workspaces … Default: `. -> target` »).
# Le sidecar RDP (rdp-sidecar/target, IronRDP + LTO), et les serveurs de test
# (test-rdp-server/target, test-vnc-server/target) sont HORS espace de travail :
# leur target n'était donc jamais caché sur GitHub, alors que .gitlab-ci.yml a
# un `cache-sidecar` dédié précisément pour cela. Chaque passage recompilait
# IronRDP, rustls et tokio depuis zéro avant le premier scénario — plusieurs
# minutes par job (e2e, e2e-windows, couverture, build, conformite, les jobs
# Windows/macOS, release, essai-windows).
#
# Ce test lit les workflows réels et exige, pour chaque job qui utilise
# rust-cache ET compile un de ces trois crates, que l'étape rust-cache déclare
# le répertoire correspondant dans `workspaces` — plus `.` lui-même, sans quoi
# renseigner `workspaces` ferait perdre le cache de `./target` (rust-cache
# n'ajoute plus l'entrée par défaut dès qu'on renseigne le champ). Contre les
# workflows d'origine (aucun `workspaces`), il échoue sur chaque job concerné.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import glob
import sys

import yaml

# Les crates hors espace de travail : chacun a son propre Cargo.lock et son
# propre répertoire target, que rust-cache ignore tant qu'on ne le nomme pas.
HORS_ESPACE = ("rdp-sidecar", "test-rdp-server", "test-vnc-server")


def workspaces_declares(etape):
    """Ensemble des racines déclarées dans le `workspaces` d'une étape rust-cache.

    Le format est une racine par ligne, éventuellement suivie de `-> cible` ;
    on ne retient que la racine (ce qui détermine le répertoire target caché).
    """
    valeur = (etape.get("with") or {}).get("workspaces")
    if not valeur:
        return set()
    racines = set()
    for ligne in str(valeur).splitlines():
        ligne = ligne.strip()
        if not ligne:
            continue
        racines.add(ligne.split("->", 1)[0].strip())
    return racines


def crates_construits(steps):
    """Crates hors espace de travail qu'un job compile réellement.

    Un crate est construit si une étape lance `cargo` en le désignant : soit par
    `working-directory`, soit en nommant son répertoire dans la commande
    (`--manifest-path <crate>/Cargo.toml`, `cd <crate>`, la boucle `for s in
    test-rdp-server test-vnc-server`). Le sidecar est aussi construit par
    scripts/couverture.sh (job Couverture), qui fait `cd rdp-sidecar && cargo
    build`.
    """
    construits = set()
    for etape in steps:
        run = etape.get("run") or ""
        wd = (etape.get("working-directory") or "").strip().strip("./")
        a_cargo = "cargo" in run
        if "couverture.sh" in run:
            construits.add("rdp-sidecar")
        if not a_cargo:
            continue
        for crate in HORS_ESPACE:
            if wd == crate or crate in run:
                construits.add(crate)
    return construits


echecs = []
verifies = []

for chemin in sorted(glob.glob(".github/workflows/*.yml")):
    workflow = yaml.safe_load(open(chemin, encoding="utf-8"))
    for nom_job, job in (workflow.get("jobs") or {}).items():
        steps = job.get("steps") or []
        cache = next(
            (
                e
                for e in steps
                if str(e.get("uses", "")).startswith("Swatinem/rust-cache")
            ),
            None,
        )
        if cache is None:
            continue
        construits = crates_construits(steps)
        if not construits:
            continue  # ce job ne compile aucun crate hors espace : rien à exiger

        declares = workspaces_declares(cache)
        # `.` doit rester listé : renseigner `workspaces` supprime l'entrée par
        # défaut `. -> target`, donc le cache du target racine.
        requis = {"."} | construits
        manquants = sorted(requis - declares)
        court = f"{chemin.split('/')[-1]}:{nom_job}"
        verifies.append((court, sorted(construits)))
        if manquants:
            echecs.append((court, manquants, sorted(construits)))

if echecs:
    for court, manquants, construits in echecs:
        print(
            f"  ✗ {court} compile {', '.join(construits)} mais son rust-cache "
            f"ne déclare pas dans `workspaces` : {', '.join(manquants)}",
            file=sys.stderr,
        )
    print(
        "  → target hors espace non caché : IronRDP/rustls/tokio recompilés "
        "depuis zéro à chaque passage (plusieurs minutes par job)",
        file=sys.stderr,
    )
    sys.exit(1)

for court, construits in verifies:
    print(f"  ✓ {court} : workspaces couvre {', '.join(construits)}")
PY
