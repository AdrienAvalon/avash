#!/usr/bin/env bash
# Contrôle reproductible : la traduction anglaise README.en.md porte la même
# section « Documentation » que README.md, avec au moins les mêmes cibles de
# liens (les chemins des documents pointés).
#
# Trouvé par l'audit du 8 septembre 2026 : README.md porte une section
# `## Documentation` (liens vers CHANGELOG, feuille-de-route, qualite,
# architecture, SECURITY, tests-parc, RELEASE) ajoutée seule côté français
# (commit af64386) ; README.en.md passait de `## Contributing` à `## License`
# sans jamais l'avoir. Un lecteur anglophone ne trouvait ni RELEASE.md ni
# tests-parc/README.md. CLAUDE.md exige « même structure, mêmes chiffres »
# entre les deux README : ce contrôle prend la section de README.md pour vérité
# et exige que README.en.md porte la sienne avec les mêmes cibles.
#
# On compare les CIBLES de liens (les chemins), pas les libellés : ceux-ci
# diffèrent forcément d'une langue à l'autre, mais les documents pointés sont
# les mêmes. README.en.md peut ajouter des cibles (jamais en retirer).
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import pathlib
import re
import sys

# Un lien Markdown en ligne : [texte](cible). On capture la cible (le chemin).
LIEN = re.compile(r"\[[^\]]*\]\(([^)]+)\)")


def cibles_section_documentation(chemin):
    """Cibles de liens listées sous le titre `## Documentation` d'un README.

    Retourne None si la section est absente, sinon l'ensemble des chemins
    (ancre et éventuelle URL exclues : on ne garde que les liens vers des
    fichiers du dépôt, comme dans la section réelle)."""
    texte = pathlib.Path(chemin).read_text(encoding="utf-8").splitlines()
    dans_section = False
    cibles = None
    for ligne in texte:
        if re.match(r"^##\s+Documentation\s*$", ligne):
            dans_section = True
            cibles = set()
            continue
        if dans_section and ligne.startswith("## "):
            # Titre suivant de même niveau : fin de la section.
            break
        if dans_section:
            for cible in LIEN.findall(ligne):
                chemin_lien = cible.split("#", 1)[0].strip()
                if not chemin_lien or chemin_lien.startswith(
                    ("http://", "https://", "//", "mailto:")
                ):
                    continue
                cibles.add(chemin_lien)
    return cibles


fr = cibles_section_documentation("README.md")
en = cibles_section_documentation("README.en.md")

if fr is None:
    print(
        "  ✗ README.md : section `## Documentation` introuvable — "
        "la source de vérité de ce contrôle a disparu",
        file=sys.stderr,
    )
    sys.exit(1)

if en is None:
    print(
        "  ✗ README.en.md : section `## Documentation` absente alors que "
        "README.md en porte une",
        file=sys.stderr,
    )
    print(
        "  → insérer une section `## Documentation` dans README.en.md "
        "(traduction des puces, « (in French) » pour les docs francophones), "
        f"reprenant les cibles : {', '.join(sorted(fr))}",
        file=sys.stderr,
    )
    sys.exit(1)

manquantes = fr - en
if manquantes:
    for cible in sorted(manquantes):
        print(
            f"  ✗ README.en.md : la section Documentation ne pointe pas `{cible}` "
            "(présent dans README.md)",
            file=sys.stderr,
        )
    print(
        "  → ajouter ces liens à la section `## Documentation` de README.en.md",
        file=sys.stderr,
    )
    sys.exit(1)

print(
    f"  ✓ README.en.md : section Documentation présente, "
    f"{len(fr)} cible(s) de README.md toutes reprises"
)
PY
