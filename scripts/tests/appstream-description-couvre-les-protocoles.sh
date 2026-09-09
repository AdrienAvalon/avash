#!/usr/bin/env bash
# Le corps de la description AppStream doit décrire le produit tel qu'il est :
# c'est ce fichier que lisent les logithèques (GNOME Logiciels, Discover) et
# Flathub via /usr/share/metainfo, et le texte détaillé est ce que lit un
# utilisateur avant d'installer.
#
# Trouvé par l'audit du 9 septembre 2026 : le résumé promettait « SSH, RDP et
# VNC » mais les deux paragraphes de <description> ne parlaient que de SSH, RDP
# et SFTP, et <keywords> ne portait pas non plus « vnc ». Le VNC est pourtant
# une fonction complète et ancienne (VeNCrypt dès la 0.3.x, cf. CHANGELOG.md),
# et le manifeste winget de la même version la présentait correctement : quelqu'un
# cherchant un client VNC dans une logithèque pouvait écarter Avash en croyant
# le mot « VNC » purement décoratif.
#
# La relecture de cette correction a trouvé la faute inverse, et plus grave : le
# texte ajouté annonçait « VNC chiffré par VeNCrypt » sans condition, alors que
# partout ailleurs (README.md, README.en.md, web/i18n.ts, CHANGELOG.md) le dépôt
# précise « quand le serveur l'offre » et avertit que sans VeNCrypt le VNC ne
# chiffre rien. Une surpromesse de sécurité dans la fiche de la logithèque est
# pire que l'omission qu'elle corrigeait, d'où le contrôle de la condition.
#
# Le contrôle ne code pas « VNC » en dur : il relit les protocoles du résumé et
# exige qu'ils reviennent dans le corps, langue par langue, et dans les mots-clés.
# Il compare ensuite les deux canaux ligne à ligne : résumé AppStream contre
# ShortDescription winget, corps AppStream contre Description winget, pour que
# la mise à jour d'un canal sans l'autre se voie.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import pathlib
import re
import sys
import xml.etree.ElementTree as ET

METAINFO = "packaging/dev.avash.app.metainfo.xml"
WINGET = pathlib.Path("packaging/winget/AdrienCros.Avash")
LANG = "{http://www.w3.org/XML/1998/namespace}lang"

# Les accès distants qu'un texte d'Avash peut citer, avec les écritures qui les
# désignent dans les deux langues. Un protocole absent de cette table ne serait
# pas contrôlé : la rallonger le jour où un canal s'ajoute.
PROTOCOLES = {
    "SSH": r"SSH",
    "RDP": r"RDP",
    "VNC": r"VNC",
    "SFTP": r"SFTP",
    "SPICE": r"SPICE",
    "Telnet": r"Telnet",
    "série": r"séri\w*|serial",
}

# Le VNC d'Avash n'est chiffré que si le serveur propose VeNCrypt ; sinon
# l'authentification VNC classique passe en clair, et le formulaire de connexion
# le dit à l'utilisateur (web/i18n.ts). Toute mention de VeNCrypt dans la fiche
# doit donc porter la même réserve que les README, mot pour mot.
CONDITION_VENCRYPT = {
    "fr": "quand le serveur l'offre",
    "en": "when the server offers it",
}

racine = ET.parse(METAINFO).getroot()

resumes = {(e.get(LANG) or "en"): (e.text or "") for e in racine.findall("summary")}
description = racine.find("description")
corps = {}
if description is not None:
    for p in description.findall("p"):
        langue = p.get(LANG) or "en"
        corps.setdefault(langue, []).append("".join(p.itertext()))

mots_cles = {(k.text or "").strip().lower() for k in racine.findall("keywords/keyword")}

echecs = []


def cites(texte):
    return {
        nom
        for nom, motif in PROTOCOLES.items()
        if re.search(rf"\b(?:{motif})\b", texte, re.IGNORECASE)
    }


if not resumes or not corps:
    echecs.append(f"{METAINFO} : résumé ou description introuvable")

attendus_toutes_langues = set()
corps_par_langue = {}
for langue, resume in sorted(resumes.items()):
    attendus = cites(resume)
    attendus_toutes_langues |= attendus
    paragraphes = corps.get(langue)
    if paragraphes is None:
        echecs.append(
            f"{METAINFO} : un résumé en « {langue} » sans description dans la "
            f"même langue"
        )
        continue
    texte = " ".join(paragraphes)
    corps_par_langue[langue] = cites(texte)
    for absent in sorted(attendus - corps_par_langue[langue]):
        echecs.append(
            f"{METAINFO} : le résumé « {langue} » annonce {absent} mais aucun "
            f"paragraphe de <description> en « {langue} » ne le mentionne ; la "
            f"fiche de la logithèque promet un protocole que son texte ignore"
        )

# La réserve sur VeNCrypt se vérifie paragraphe par paragraphe : c'est la phrase
# qui affirme le chiffrement qui doit la porter, pas un autre bout du texte.
for langue, paragraphes in sorted(corps.items()):
    for paragraphe in paragraphes:
        if not re.search(r"VeNCrypt", paragraphe, re.IGNORECASE):
            continue
        condition = CONDITION_VENCRYPT.get(langue)
        if condition is None:
            echecs.append(
                f"{METAINFO} : un paragraphe en « {langue} » cite VeNCrypt mais "
                f"aucune formulation conditionnelle n'est connue pour cette "
                f"langue ; ajouter la réserve des README à CONDITION_VENCRYPT"
            )
            continue
        if condition.lower() not in paragraphe.lower():
            echecs.append(
                f"{METAINFO} : un paragraphe en « {langue} » annonce le "
                f"chiffrement VeNCrypt sans la réserve « {condition} » ; le VNC "
                f"n'est chiffré que si le serveur le propose (README.md, "
                f"web/i18n.ts), et la fiche de la logithèque promettrait une "
                f"sécurité que l'application dément"
            )

# Les mots-clés servent la recherche des logithèques : chercher « vnc » doit
# ramener Avash, sinon la fiche existe mais reste introuvable.
for proto in sorted(attendus_toutes_langues):
    if proto.lower() not in mots_cles:
        echecs.append(
            f"{METAINFO} : {proto} est annoncé dans le résumé mais absent de "
            f"<keywords> ; une recherche « {proto.lower()} » ne trouve pas Avash"
        )

# Winget décrit le même produit : une divergence de liste signale que l'un des
# deux canaux a été mis à jour sans l'autre. Seul le manifeste le plus récent
# est contrôlé ; les dossiers des versions passées sont des archives publiées
# telles quelles, que personne ne réécrit. Le plus récent, et non celui de la
# version annoncée par <releases> : le manifeste winget se génère depuis
# l'installeur de la release GitHub (scripts/winget-manifeste.sh), donc APRÈS
# le tag, alors que la version est portée dans metainfo AVANT lui (RELEASE.md
# §7). Exiger le manifeste de la version courante bloquait le commit de version
# lui-même (vu à la 0.11.0, le 9 septembre 2026) ; le commit des canaux qui suit
# la publication le fait entrer dans le contrôle. Ce qui compte pour l'intention
# de ce garde, c'est que le dernier texte winget publié dise la même chose que
# la fiche AppStream.
def cle_version(v):
    return tuple(int(n) for n in re.findall(r"\d+", v))


dossiers = [d for d in WINGET.iterdir() if d.is_dir()] if WINGET.is_dir() else []
courant = max(dossiers, key=lambda d: cle_version(d.name), default=None)
locales = sorted(courant.glob("*.locale.*.yaml")) if courant else []
if courant is None:
    echecs.append(
        f"{WINGET} ne contient aucun dossier de version : aucun manifeste "
        f"winget à comparer à {METAINFO}"
    )
elif not locales:
    echecs.append(
        f"{courant} ne contient aucun fichier *.locale.*.yaml : les textes "
        f"winget de la version courante manquent, rien n'a pu être comparé"
    )


def bloc_description(texte):
    trouve = re.search(
        r"^Description:\s*\|-?\s*\n((?:[ \t]+.*(?:\n|$))+)", texte, re.MULTILINE
    )
    return trouve.group(1) if trouve else None


for locale in locales:
    texte = locale.read_text(encoding="utf-8")
    langue = locale.name.split(".locale.")[1].split(".yaml")[0].split("-")[0]
    court = re.search(r"^ShortDescription:\s*(.+)$", texte, re.MULTILINE)
    if court is None:
        echecs.append(f"{locale} : pas de ShortDescription")
    else:
        annonces = cites(court.group(1))
        for manquant in sorted(annonces - attendus_toutes_langues):
            echecs.append(
                f"{locale} annonce {manquant} alors que le résumé AppStream de "
                f"{METAINFO} ne le cite pas : les deux canaux divergent"
            )
        for manquant in sorted(attendus_toutes_langues - annonces):
            echecs.append(
                f"{METAINFO} annonce {manquant} alors que le ShortDescription de "
                f"{locale} ne le cite pas : les deux canaux divergent"
            )

    # Le texte long de winget est le pendant du corps de <description> : c'est
    # là que la première correction laissait encore les consoles série côté
    # winget seulement, sans que rien ne le signale.
    long = bloc_description(texte)
    if long is None:
        echecs.append(f"{locale} : pas de bloc Description")
        continue
    if langue not in corps_par_langue:
        echecs.append(
            f"{locale} est en « {langue} » mais {METAINFO} n'a pas de "
            f"<description> dans cette langue : rien à comparer"
        )
        continue
    annonces = cites(long)
    for manquant in sorted(annonces - corps_par_langue[langue]):
        echecs.append(
            f"{locale} décrit {manquant} dans sa Description alors que les "
            f"paragraphes « {langue} » de {METAINFO} l'ignorent : la fiche de "
            f"la logithèque décrit un produit plus étroit que celui de winget"
        )
    for manquant in sorted(corps_par_langue[langue] - annonces):
        echecs.append(
            f"{METAINFO} décrit {manquant} en « {langue} » alors que la "
            f"Description de {locale} l'ignore : les deux canaux divergent"
        )

if echecs:
    for e in echecs:
        print("  ✗", e, file=sys.stderr)
    sys.exit(1)

print(
    "  ✓ description AppStream, mots-clés et winget couvrent "
    + ", ".join(sorted(attendus_toutes_langues))
    + " ; la mention VeNCrypt reste conditionnelle"
)
PY
