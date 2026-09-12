#!/usr/bin/env bash
# Contrôle reproductible : chaque commande cargo qui résout les dépendances
# (build, test, check, clippy, run, tauri build, deny, mutants) porte --locked,
# dans les trois chaînes, check.sh, le hook de pré-commit et les scripts.
#
# Trouvé par l'audit du 12 septembre 2026 (C-chaine-4) : aucune des 66
# commandes cargo de build, de test ou de clippy ne portait --locked. Sans lui,
# cargo re-résout et réécrit Cargo.lock sur l'exécuteur dès qu'un manifeste ne
# concorde plus, pendant que le SBOM attesté de la release décrit le
# Cargo.lock commité : les binaires pouvaient embarquer des versions que
# personne n'avait relues, sous une nomenclature qui en décrivait d'autres. Le
# même trou masquait un verrou oublié lors d'une montée de version (RELEASE.md,
# builds --frozen de l'AUR et de Flathub).
#
# Règles : `--locked` avant le premier ` -- ` (après, il irait à clippy ou au
# programme lancé) ; pour `cargo tauri build`, après ` -- `, où tauri-cli passe
# les arguments à cargo ; pour cargo-mutants, `--cargo-arg=--locked`.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import glob
import re
import sys

fichiers = sorted(
    glob.glob(".github/workflows/*.yml")
    + [".gitlab-ci.yml", "check.sh", "scripts/hooks/pre-commit"]
    + glob.glob("scripts/*.sh")
)

CARGO = re.compile(r"\bcargo(?:\s+\+\S+)?\s+(build|test|check|clippy|run)\b")
TAURI = re.compile(r"\bcargo\s+tauri\s+build\b")
DENY = re.compile(r"\bcargo\s+deny\b")
MUTANTS = re.compile(r"\bcargo\s+mutants\b")
FIN = re.compile(r"\s(?:&&|\|\||\||;)\s|\s(?:&&|\|\|)$")


def lignes_logiques(texte):
    """Lignes du fichier, continuations `\\` jointes, avec leur numéro."""
    tampon, debut = "", None
    for n, ligne in enumerate(texte.splitlines(), 1):
        if debut is None:
            debut = n
        if ligne.rstrip().endswith("\\"):
            tampon += ligne.rstrip()[:-1] + " "
            continue
        yield debut, tampon + ligne
        tampon, debut = "", None
    if tampon:
        yield debut, tampon


def segment(ligne, depart):
    # Une expression `${{ … }}` de GitHub porte ses propres && et || : elle ne
    # termine pas la commande.
    reste = re.sub(r"\$\{\{.*?\}\}", "EXPR", ligne[depart:])
    m = FIN.search(reste)
    return reste[: m.start()] if m else reste


echecs, vus = [], 0
for chemin in fichiers:
    try:
        texte = open(chemin, encoding="utf-8").read()
    except FileNotFoundError:
        continue
    for n, ligne in lignes_logiques(texte):
        nue = ligne.strip()
        # Commentaires, et noms d'étapes YAML (« cargo check (crate avash) »).
        if nue.startswith("#") or re.match(r"(-\s*)?name:", nue):
            continue
        # Les messages d'aide (echo, printf) citent une commande sans la lancer.
        for rx, genre in ((TAURI, "tauri"), (CARGO, "cargo"), (DENY, "deny"), (MUTANTS, "mutants")):
            for m in rx.finditer(ligne):
                avant = ligne[: m.start()]
                if re.search(r"\b(echo|printf)\b", avant):
                    continue
                if genre == "cargo" and TAURI.match(ligne, m.start()):
                    continue
                seg = segment(ligne, m.start())
                vus += 1
                avant_sep, _, apres_sep = seg.partition(" -- ")
                if genre == "deny" and not re.search(r"\scheck\b", seg):
                    vus -= 1  # `cargo deny --version` ne résout rien
                    continue
                if genre == "tauri":
                    ok = "--locked" in apres_sep.split()
                elif genre == "mutants":
                    ok = re.search(r"--cargo-arg[= ]--locked\b|-C\s*--locked\b", seg) is not None
                else:
                    ok = "--locked" in avant_sep.split()
                if not ok:
                    echecs.append(f"{chemin}:{n} : {seg.strip()[:110]}")

if echecs:
    print("  ✗ commandes cargo sans --locked (cargo réécrirait Cargo.lock en silence) :", file=sys.stderr)
    for e in echecs:
        print(f"      {e}", file=sys.stderr)
    sys.exit(1)
print(f"  ✓ {vus} commandes cargo, toutes en --locked")
PY
