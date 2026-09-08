#!/usr/bin/env bash
# Contrôle reproductible : les listes de fichiers passées aux actions de
# publication et d'attestation ne contiennent pas de ligne de commentaire.
#
# Trouvé par l'audit du 8 septembre 2026 : dans un scalaire bloc `|`, une ligne
# commençant par `#` n'est PAS un commentaire YAML, elle fait partie de la
# valeur. Le bloc `files: |` de softprops/action-gh-release portait cinq lignes
# « # Un seul motif pour les signatures… » : l'action les recevait comme des
# motifs de fichiers, les découpait sur les retours à la ligne et les virgules
# et lançait `glob.sync` sur chaque fragment (« et la », « seconde copie »).
# Aujourd'hui ces motifs ne correspondent à rien et sont ignorés (bruit
# « unmatched files » dans les logs) ; le jour où l'action activerait
# `fail_on_unmatched_files` ou refuserait un motif sans correspondance, la
# publication échouerait sur un commentaire.
#
# Ce test lit les workflows réels et exige, pour chaque champ qui reçoit une
# liste de fichiers (`files`, `subject-path`), qu'aucune de ses lignes ne
# commence par `#`. Le commentaire doit vivre au-dessus de la clé, au niveau
# d'indentation YAML. Contre le workflow d'origine, il échoue sur `release.yml`.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import glob
import sys

import yaml

# Champs `with` dont la valeur est une liste de motifs de fichiers, une par
# ligne : un `#` en tête de ligne y est de la donnée, pas un commentaire.
CHAMPS_LISTE = ("files", "subject-path")

echecs = []
verifies = 0

for chemin in sorted(glob.glob(".github/workflows/*.yml")):
    workflow = yaml.safe_load(open(chemin, encoding="utf-8"))
    for nom_job, job in (workflow.get("jobs") or {}).items():
        for etape in job.get("steps") or []:
            avec = etape.get("with") or {}
            for champ in CHAMPS_LISTE:
                valeur = avec.get(champ)
                if not isinstance(valeur, str):
                    continue
                verifies += 1
                commentaires = [
                    ligne.strip()
                    for ligne in valeur.splitlines()
                    if ligne.strip().startswith("#")
                ]
                if commentaires:
                    court = f"{chemin.split('/')[-1]}:{nom_job}:{champ}"
                    echecs.append((court, commentaires))

if echecs:
    for court, commentaires in echecs:
        print(
            f"  ✗ {court} : le bloc contient {len(commentaires)} ligne(s) "
            f"commençant par `#`, transmises comme motifs de fichiers",
            file=sys.stderr,
        )
        for ligne in commentaires:
            print(f"      {ligne}", file=sys.stderr)
    print(
        "  → sortir le commentaire du bloc `|` : le placer au-dessus de la clé, "
        "au niveau d'indentation YAML",
        file=sys.stderr,
    )
    sys.exit(1)

print(f"  ✓ {verifies} liste(s) de fichiers, aucune ligne de commentaire dans un bloc")
PY
