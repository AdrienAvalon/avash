#!/usr/bin/env bash
# Contrôle reproductible : le point « Flathub » de docs/feuille-de-route.md ne
# cite pas un tag de manifeste périmé.
#
# Trouvé par l'audit du 9 septembre 2026 : le point annonçait le manifeste
# « construit, installé et lancé sur le poste par flatpak-builder, depuis le
# tag v0.8.0 » alors que `packaging/flathub/io.github.AdrienAvalon.avash.yml`
# pointe `tag: v0.10.1` depuis les publications 0.9.x et 0.10.x. Le lecteur qui
# ouvre la feuille de route pour savoir où en est le canal Flathub en déduit que
# le manifeste réellement éprouvé est celui d'aujourd'hui, alors que ce qui a
# été construit et lancé sur le poste est une version antérieure du manifeste :
# il croit la soumission plus mûre qu'elle ne l'est et n'a aucune raison de
# rejouer `flatpak-builder --install` avant d'ouvrir la PR.
#
# La règle tenue ici : si ce point cite le moindre tag `vX.Y.Z`, le tag courant
# du manifeste doit être parmi eux, pour que la prose dise toujours où pointe le
# manifeste d'aujourd'hui. Une rédaction sans aucun numéro de version reste
# permise (c'est le parti pris ailleurs dans le document), mais un numéro cité
# doit rester confrontable au manifeste : à la prochaine publication qui bouge
# le tag, ce contrôle rougit et force la mise à jour de la phrase.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

MANIFESTE="packaging/flathub/io.github.AdrienAvalon.avash.yml"
FEUILLE="docs/feuille-de-route.md"

python3 - "$MANIFESTE" "$FEUILLE" <<'PY'
import re
import sys

chemin_manifeste, chemin_feuille = sys.argv[1:3]


def lire(chemin):
    return open(chemin, encoding="utf-8").read()


manifeste = lire(chemin_manifeste)
feuille = lire(chemin_feuille)

echecs = []

# Le tag de la source git du manifeste. Relu textuellement : l'assertion l'est
# aussi, et le contrôle doit tourner sur un poste sans PyYAML.
tags_manifeste = re.findall(r"^\s*tag:\s*['\"]?(v[0-9][^'\"\s]*)", manifeste, re.M)
if not tags_manifeste:
    echecs.append(
        f"{chemin_manifeste} : aucune ligne « tag: vX.Y.Z » lisible, le tag "
        "annoncé par la feuille de route n'est plus confrontable au manifeste"
    )

# Le point « Flathub » de l'axe distribution, jusqu'au point de liste suivant.
debut = feuille.find("\n- **Flathub**")
if debut == -1:
    echecs.append(
        f"{chemin_feuille} : point « Flathub » introuvable, le tag qu'il "
        "annonce n'est plus vérifiable"
    )
    point = ""
else:
    fin = feuille.find("\n- **", debut + 1)
    point = feuille[debut : fin if fin != -1 else len(feuille)]

tags_cites = re.findall(r"\bv[0-9]+\.[0-9]+\.[0-9]+\b", point)
if tags_cites and tags_manifeste:
    courant = tags_manifeste[0]
    if courant not in tags_cites:
        echecs.append(
            f"{chemin_feuille} (point Flathub) cite le ou les tags "
            + ", ".join(sorted(set(tags_cites)))
            + f" mais jamais {courant}, le tag vers lequel pointe aujourd'hui "
            f"{chemin_manifeste} : le lecteur croit éprouvé un manifeste qui a "
            "changé de version depuis"
        )

if echecs:
    for e in echecs:
        print("  ✗", e, file=sys.stderr)
    sys.exit(1)

print(
    "  ✓ docs/feuille-de-route.md (point Flathub) : le tag cité est celui vers "
    "lequel pointe le manifeste"
)
PY
