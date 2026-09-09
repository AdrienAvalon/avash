#!/usr/bin/env bash
# Contrôle reproductible du compte des droits Flathub à justifier, partout où
# le dépôt l'annonce : RELEASE.md §8 (annonce, énumération, instruction finale
# au mainteneur) et le point « Flathub » de docs/feuille-de-route.md, qui
# décrit la même soumission et renvoie à RELEASE.md. Les deux documents
# doivent citer les mêmes droits, en même nombre, et chacun de ces droits doit
# exister dans les finish-args du manifeste.
#
# Trouvé par l'audit du 9 septembre 2026 : le paragraphe de RELEASE.md
# annonçait « cinq droits qui demandent une exception » et les listait bien
# tous les cinq, mais sa phrase de clôture demandait encore « les trois
# exceptions », et la feuille de route parlait elle aussi de « trois droits
# (agent SSH, dossier personnel, nom D-Bus de Tauri) » : le compte d'avant
# l'ajout de `--socket=pulseaudio` et `--device=all` par l'audit du
# 7 septembre. Le mainteneur qui suit l'un ou l'autre document pour ouvrir la
# PR chez `flathub/flathub` ne justifie alors que trois droits et laisse sans
# explication les deux plus larges (serveur audio, accès à tous les
# périphériques), que le robot ou un reviewer Flathub bloque.
#
# Le contrôle voisin `flathub-permissions-audio-serie.sh` vérifie seulement
# que ces deux droits apparaissent quelque part dans RELEASE.md, pas que la
# prose compte juste ni que les autres documents suivent : c'est ce trou-là
# que ce test ferme.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

MANIFESTE="packaging/flathub/io.github.AdrienAvalon.avash.yml"
RELEASE="RELEASE.md"
FEUILLE="docs/feuille-de-route.md"

python3 - "$MANIFESTE" "$RELEASE" "$FEUILLE" <<'PY'
import re
import sys

chemin_manifeste, chemin_release, chemin_feuille = sys.argv[1:4]

# Les nombres sont écrits en toutes lettres dans la prose française du dépôt.
NOMBRES = {
    "un": 1, "une": 1, "deux": 2, "trois": 3, "quatre": 4, "cinq": 5,
    "six": 6, "sept": 7, "huit": 8, "neuf": 9, "dix": 10,
}


def lire(chemin):
    return open(chemin, encoding="utf-8").read()


def finish_args(chemin):
    """Les droits accordés par le manifeste.

    L'assertion visée par ce contrôle est purement textuelle, donc il doit
    tourner même sur un poste sans PyYAML : à défaut de la bibliothèque, les
    finish-args se relisent à la main, c'est une liste plate de scalaires.
    """
    texte = lire(chemin)
    try:
        import yaml
    except ImportError:
        pass
    else:
        return yaml.safe_load(texte).get("finish-args") or []
    bloc = re.search(r"^finish-args:[ \t]*\n((?:(?:[ \t]+.*)?\n)+)", texte, re.M)
    if bloc is None:
        return []
    return re.findall(r"^[ \t]*-[ \t]*[\"']?(--[^\"'\s]+)", bloc.group(1), re.M)


def droits_cites(fragment):
    """Les droits énumérés dans un fragment de prose, dans l'ordre d'apparition.

    Sans déduplication : un droit cité deux fois gonfle le compte énuméré et
    doit se voir, sinon un écart réel se cacherait derrière un doublon.
    """
    return re.findall(r"`(--[a-z0-9-]+=[^`]+)`", fragment)


echecs = []

manifeste = finish_args(chemin_manifeste)
if not manifeste:
    echecs.append(
        f"{chemin_manifeste} : finish-args illisible ou vide, les droits cités "
        "par la documentation ne sont plus confrontables au manifeste"
    )

# --- RELEASE.md §8 : annonce, énumération, rappel final au mainteneur -------
release = lire(chemin_release)
annonce = re.search(
    r"Le\s+linter\s+signale\s+(\S+)\s+droits?\s+qui\s+demandent?\s+une\s+exception",
    release,
)
cloture = re.search(r"demander\s+les\s+(\S+)\s+exceptions", release)

if annonce is None:
    echecs.append(
        "RELEASE.md §8 : phrase « Le linter signale N droits qui demandent "
        "une exception » introuvable, le compte annoncé n'est plus vérifiable"
    )
if cloture is None:
    echecs.append(
        "RELEASE.md §8 : instruction finale « demander les N exceptions » "
        "introuvable, le compte rappelé au mainteneur n'est plus vérifiable"
    )

droits_release = []
if annonce is not None:
    # Les droits énumérés vivent entre l'annonce et le paragraphe qui enchaîne
    # sur les mises à jour ultérieures.
    fin = release.find("Les mises à jour suivantes", annonce.end())
    if fin == -1:
        fin = cloture.start() if cloture is not None else len(release)
    droits_release = droits_cites(release[annonce.end():fin])

# --- docs/feuille-de-route.md : le point « Flathub » de l'axe distribution --
feuille = lire(chemin_feuille)
annonce_feuille = re.search(
    r"le\s+linter\s+Flathub\s+passe\s+hormis\s+(\S+)\s+droits?", feuille
)

droits_feuille = []
if annonce_feuille is None:
    echecs.append(
        "docs/feuille-de-route.md : point « Flathub », phrase « le linter "
        "Flathub passe hormis N droits » introuvable, le compte annoncé par "
        "la feuille de route n'est plus vérifiable"
    )
else:
    # Le point de liste court jusqu'au suivant, qui commence en début de ligne.
    fin = feuille.find("\n- **", annonce_feuille.end())
    if fin == -1:
        fin = len(feuille)
    droits_feuille = droits_cites(feuille[annonce_feuille.end():fin])


def controler(etiquette, mot, droits):
    """Le nombre annoncé, le nombre énuméré et le manifeste doivent s'accorder."""
    compte = NOMBRES.get(mot)
    if compte is None:
        echecs.append(
            f"{etiquette} : « {mot} droits » n'est pas un nombre écrit en "
            "toutes lettres reconnu"
        )
        return None
    if compte != len(droits):
        echecs.append(
            f"{etiquette} annonce {mot} ({compte}) droits à justifier mais en "
            f"énumère {len(droits)} : " + (", ".join(droits) or "aucun")
        )
    doublons = sorted({d for d in droits if droits.count(d) > 1})
    if doublons:
        echecs.append(
            f"{etiquette} cite deux fois " + ", ".join(doublons)
            + " : le compte énuméré ne veut plus rien dire"
        )
    for droit in droits:
        if manifeste and droit not in manifeste:
            echecs.append(
                f"{etiquette} cite {droit} parmi les exceptions à justifier, "
                "mais ce droit est absent des finish-args du manifeste"
            )
    return compte


compte_release = None
if annonce is not None:
    compte_release = controler("RELEASE.md §8", annonce.group(1), droits_release)
if annonce_feuille is not None:
    controler(
        "docs/feuille-de-route.md (point Flathub)",
        annonce_feuille.group(1),
        droits_feuille,
    )

if compte_release is not None and cloture is not None:
    mot_cloture = cloture.group(1)
    compte_cloture = NOMBRES.get(mot_cloture)
    if compte_cloture is None:
        echecs.append(
            f"RELEASE.md §8 : « les {mot_cloture} exceptions » n'est pas un "
            "nombre écrit en toutes lettres reconnu"
        )
    elif compte_cloture != compte_release:
        echecs.append(
            f"RELEASE.md §8 annonce {annonce.group(1)} ({compte_release}) "
            "droits à justifier mais l'instruction finale de la PR de "
            f"soumission n'en demande que {mot_cloture} ({compte_cloture}) : "
            "le mainteneur qui suit le document laisse des droits sans "
            "justification"
        )

# Les deux documents décrivent la même soumission : ils doivent citer les mêmes
# droits, pas seulement le même nombre.
if droits_release and droits_feuille and set(droits_release) != set(droits_feuille):
    manquants = sorted(set(droits_release) - set(droits_feuille))
    en_trop = sorted(set(droits_feuille) - set(droits_release))
    detail = []
    if manquants:
        detail.append("absents de la feuille de route : " + ", ".join(manquants))
    if en_trop:
        detail.append("absents de RELEASE.md : " + ", ".join(en_trop))
    echecs.append(
        "RELEASE.md §8 et docs/feuille-de-route.md décrivent la même "
        "soumission Flathub mais pas les mêmes droits à justifier ("
        + " ; ".join(detail) + ")"
    )

if echecs:
    for e in echecs:
        print("  ✗", e, file=sys.stderr)
    sys.exit(1)

print(
    "  ✓ droits Flathub à justifier : RELEASE.md §8 (annonce, énumération, "
    "rappel final) et docs/feuille-de-route.md concordent entre eux et avec "
    "le manifeste"
)
PY
