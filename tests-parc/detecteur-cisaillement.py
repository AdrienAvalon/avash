#!/usr/bin/env python3
"""Détecte un cisaillement horizontal dans une capture d'écran RDP.

Pourquoi ce détecteur existe : le décodeur d'IronRDP écrivait les tuiles à la
largeur du bitmap et les relisait à celle du rectangle. Chaque ligne glissait de
la différence. L'image restait « plausible » — bonnes couleurs, bonne
disposition générale — donc aucune vérification grossière (taille, somme de
contrôle, moyenne des pixels) ne l'aurait vue. C'est Adrien qui l'a vue, à
l'œil.

Le principe : une image d'interface a de larges aplats, donc deux lignes
voisines se superposent au mieux avec un décalage NUL. Sous cisaillement,
l'alignement optimal devient une constante non nulle, sur la quasi-totalité des
lignes. Mesuré sur de vraies captures : 0 sur 100 % des lignes quand c'est sain,
-2 sur 96 % quand ça ne l'est pas. La séparation est franche.

Usage : detecteur-cisaillement.py capture.png [autre.png …]
Sortie : code 1 si au moins une image est cisaillée.
"""
import sys
import numpy as np
from PIL import Image

AMPLITUDE = 6      # décalages testés, en pixels
ECHANTILLON = 140  # lignes examinées
PART_MIN = 0.30    # proportion de lignes concordantes pour conclure


def decalage_modal(chemin, echantillon=ECHANTILLON, amplitude=AMPLITUDE):
    """Décalage le plus fréquent entre lignes voisines, et sa proportion."""
    a = np.asarray(Image.open(chemin).convert("L"), dtype=np.int16)
    h, _ = a.shape
    votes = []
    for y in np.linspace(1, h - 1, min(echantillon, h - 1), dtype=int):
        haut, bas = a[y - 1], a[y]
        if haut.std() < 4:  # aplat uniforme : n'apprend rien
            continue
        meilleur, score = 0, None
        for d in range(-amplitude, amplitude + 1):
            if d < 0:
                diff = np.abs(haut[-d:] - bas[:d]).mean()
            elif d > 0:
                diff = np.abs(haut[:-d] - bas[d:]).mean()
            else:
                diff = np.abs(haut - bas).mean()
            if score is None or diff < score:
                score, meilleur = diff, d
        votes.append(meilleur)
    if not votes:
        return 0, 0.0
    vals, cpt = np.unique(votes, return_counts=True)
    i = int(cpt.argmax())
    return int(vals[i]), float(cpt[i]) / len(votes)


def main(chemins):
    mauvais = 0
    for c in chemins:
        d, part = decalage_modal(c)
        cisaillee = d != 0 and part > PART_MIN
        mauvais += cisaillee
        etat = "CISAILLÉE" if cisaillee else "saine"
        print(f"  {etat:10} décalage={d:+d} ({part:.0%} des lignes)  {c}")
    return 1 if mauvais else 0


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    sys.exit(main(sys.argv[1:]))
