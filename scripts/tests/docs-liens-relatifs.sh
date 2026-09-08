#!/usr/bin/env bash
# Contrôle reproductible : les liens relatifs des documents Markdown de la
# racine (CONTRIBUTING.md, SECURITY.md, RELEASE.md, README*.md, CHANGELOG.md)
# pointent vers un fichier qui existe, résolu DEPUIS le dossier du document.
#
# Trouvé par l'audit du 8 septembre 2026 : le tableau d'outillage de
# CONTRIBUTING.md renvoyait à `[tests-parc](../tests-parc/README.md)`. Le
# document étant à la racine, `../` sort du dépôt : sur GitHub le lien mène à
# une 404, alors que le bon chemin — `tests-parc/README.md` — est déjà utilisé
# plus bas dans le même fichier. Le contrôle résout chaque cible relative
# depuis le dossier du document et échoue si elle n'existe pas.
#
# Ne sont vérifiés que les liens vers des fichiers du dépôt : les URL (http, //,
# mailto) et les ancres pures (`#section`) sont ignorées ; une éventuelle ancre
# `chemin#ancre` est tronquée avant résolution.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import pathlib
import re
import sys

# Un lien Markdown en ligne : [texte](cible). On capture la cible.
LIEN = re.compile(r"\[[^\]]*\]\(([^)]+)\)")

racine = pathlib.Path(".")
documents = [
    "CONTRIBUTING.md",
    "SECURITY.md",
    "RELEASE.md",
    "CHANGELOG.md",
    "README.md",
    "README.en.md",
]

echecs = []
verifies = 0

for nom in documents:
    doc = racine / nom
    if not doc.exists():
        continue
    dossier = doc.parent
    for no_ligne, ligne in enumerate(doc.read_text(encoding="utf-8").splitlines(), 1):
        for cible in LIEN.findall(ligne):
            cible = cible.strip()
            # URL absolue, protocole relatif, ancre pure : hors sujet.
            if (
                cible.startswith(("http://", "https://", "//", "mailto:", "#"))
                or ":" in cible.split("/", 1)[0]
            ):
                continue
            # `chemin#ancre` → on ne résout que le chemin.
            chemin = cible.split("#", 1)[0]
            if not chemin:
                continue
            verifies += 1
            if not (dossier / chemin).exists():
                echecs.append((nom, no_ligne, cible))

if echecs:
    for nom, no_ligne, cible in echecs:
        print(
            f"  ✗ {nom}:{no_ligne} : lien relatif cassé `{cible}` "
            f"(cible introuvable depuis {nom})",
            file=sys.stderr,
        )
    print(
        "  → corriger le chemin pour qu'il se résolve depuis le dossier du "
        "document (à la racine, pas de `../`)",
        file=sys.stderr,
    )
    sys.exit(1)

print(f"  ✓ {verifies} lien(s) relatif(s) de doc racine, toutes cibles présentes")
PY
