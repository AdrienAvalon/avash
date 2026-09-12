#!/usr/bin/env bash
# Contrôle reproductible : la clé privée de la mise à jour automatique n'entre
# jamais dans l'environnement d'un programme qui compile ou installe du code
# tiers.
#
# Trouvé par l'audit du 12 septembre 2026 (C-chaine-1) : l'étape « Bundles
# Tauri » de release.yml recevait TAURI_SIGNING_PRIVATE_KEY alors que `cargo
# tauri build` lance vite et ses greffons (beforeBuildCommand), puis chaque
# build.rs et chaque macro procédurale de l'espace de travail, et ne signe qu'à
# la fin. N'importe laquelle de ces dépendances lisait la clé par un simple
# `std::env::var`, et avec elle signait une mise à jour que tous les postes
# installés auraient acceptée, sans révocation possible. essai-windows.yml
# faisait de même pour un .sig jeté, et scripts/release.sh exportait la clé
# avant `cargo tauri build`.
#
# Règles vérifiées :
#   1. aucun `env:` de workflow ou de job ne porte la clé ;
#   2. un job qui la reçoit ne compile rien, n'installe rien, n'utilise que les
#      actions d'artefacts officielles, et seule l'étape qui appelle
#      `signer sign` la reçoit ;
#   3. ce job signe les cinq artefacts de mise à jour (AppImage, deb, rpm,
#      setup NSIS, archive .app.tar.gz) ;
#   4. chaque `cargo tauri build` coupe createUpdaterArtifacts (sans quoi Tauri
#      exige la clé pour construire) ;
#   5. scripts/release.sh n'exporte pas la clé : il la passe à `signer sign`
#      seul, après la construction.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import glob
import re
import sys

import yaml

CLE = "TAURI_SIGNING_PRIVATE_KEY"
# Ce qui exécute du code tiers : compilation, installation, gestionnaires de
# paquets, interpréteurs de scripts de projet.
INTERDIT = re.compile(
    r"\bcargo\s+(?:\+\S+\s+)?(?:build|test|check|clippy|run|install|fuzz|llvm-cov|mutants|tauri\s+build)\b"
    r"|\bnpm\b|\bnpx\b|\byarn\b|\bpnpm\b|\bpip3?\b|\bmake\b|\brustup\b|\bapt-get\b|\bbrew\b"
)
ACTIONS_PERMISES = ("actions/download-artifact@", "actions/upload-artifact@")
ARTEFACTS_MAJ = ("*.AppImage", "*.deb", "*.rpm", "*-setup.exe", "*.app.tar.gz")

echecs = []


def porte_cle(bloc):
    return isinstance(bloc, dict) and any(CLE in str(k) or CLE in str(v) for k, v in bloc.items())


jobs_a_cle = []
for chemin in sorted(glob.glob(".github/workflows/*.yml")):
    nom_wf = chemin.split("/")[-1]
    wf = yaml.safe_load(open(chemin, encoding="utf-8"))
    if porte_cle(wf.get("env")):
        echecs.append(f"{nom_wf} : la clé est dans l'env du workflow, donc de chaque étape")
    for nom_job, job in (wf.get("jobs") or {}).items():
        court = f"{nom_wf}:{nom_job}"
        texte_job = yaml.safe_dump(job, allow_unicode=True)
        if CLE not in texte_job:
            continue
        jobs_a_cle.append(court)
        if porte_cle(job.get("env")):
            echecs.append(f"{court} : la clé est dans l'env du job, donc de chaque étape")
        etapes = job.get("steps") or []
        signe = ""
        for etape in etapes:
            uses = str(etape.get("uses", ""))
            run = str(etape.get("run", ""))
            nom = etape.get("name") or uses.split("@")[0] or (run.splitlines()[0] if run else "?")
            if uses and not uses.startswith(ACTIONS_PERMISES):
                echecs.append(f"{court} : l'étape « {nom} » exécute l'action tierce {uses.split('@')[0]} dans le job qui détient la clé")
            if INTERDIT.search(run):
                echecs.append(f"{court} : l'étape « {nom} » compile ou installe du code tiers dans le job qui détient la clé")
            if porte_cle(etape.get("env")) or porte_cle(etape.get("with")):
                if "signer sign" not in run:
                    echecs.append(f"{court} : l'étape « {nom} » reçoit la clé sans être l'étape de signature")
                signe += run
        manquants = [a for a in ARTEFACTS_MAJ if a not in signe]
        if manquants:
            echecs.append(f"{court} : la signature ne couvre pas {', '.join(manquants)}")

    for nom_job, job in (wf.get("jobs") or {}).items():
        for etape in job.get("steps") or []:
            run = str(etape.get("run", ""))
            if re.search(r"\bcargo\s+tauri\s+build\b", run) and '"createUpdaterArtifacts":false' not in run.replace(" ", ""):
                echecs.append(f"{nom_wf}:{nom_job} : `cargo tauri build` sans --config coupant createUpdaterArtifacts (Tauri réclamerait la clé)")

if not any(j.startswith("release.yml:") for j in jobs_a_cle):
    echecs.append("release.yml : aucun job ne signe les artefacts de mise à jour")
autres = [j for j in jobs_a_cle if not j.startswith("release.yml:")]
if autres:
    echecs.append(f"la clé est citée hors de release.yml : {', '.join(autres)}")

# scripts/release.sh : la clé ne doit jamais être exportée ni posée devant la
# construction ; seule la commande `signer sign` la lit, par son chemin.
release = open("scripts/release.sh", encoding="utf-8").read()
for n, ligne in enumerate(release.splitlines(), 1):
    code = ligne.split("#", 1)[0]
    if re.search(r"\bexport\s+TAURI_SIGNING_PRIVATE_KEY\b", code) or re.search(r"^\s*TAURI_SIGNING_PRIVATE_KEY=", code):
        echecs.append(f"scripts/release.sh:{n} : la clé est exportée, donc visible de `cargo tauri build`")
    if re.search(r"\bcargo\s+tauri\s+build\b", code):
        if CLE in code:
            echecs.append(f"scripts/release.sh:{n} : la clé est passée à `cargo tauri build`")
        if '"createUpdaterArtifacts":false' not in code.replace(" ", ""):
            echecs.append(f"scripts/release.sh:{n} : `cargo tauri build` sans --config coupant createUpdaterArtifacts")
if "signer sign" not in release:
    echecs.append("scripts/release.sh : aucune signature après la construction (`signer sign`)")

if echecs:
    for e in echecs:
        print(f"  ✗ {e}", file=sys.stderr)
    sys.exit(1)
print(f"  ✓ la clé de signature n'atteint que {', '.join(jobs_a_cle)}, qui ne compile ni n'installe rien ; release.sh signe après construction")
PY
