#!/usr/bin/env bash
# Contrôle reproductible : la règle des « portes » de CONTRIBUTING.md ne
# sur-promet pas le hook de pré-commit, et les portes qu'elle déclare
# obligatoires jouent réellement les tests des crates hors espace de travail.
#
# Trouvé par l'audit du 8 septembre 2026 : CONTRIBUTING.md énonçait « Toute
# crate ajoutée hors du workspace doit être branchée explicitement sur les
# QUATRE portes — check.sh, le hook de pré-commit, ci.yml et .gitlab-ci.yml ».
# Or le hook (`scripts/hooks/pre-commit`), voulu « barrière rapide », ne joue
# que le workspace et le processus RDP : il ne lance PAS les tests de
# test-rdp-server (27 tests) ni de test-vnc-server (2 tests). Un contributeur
# qui casse un décodeur du serveur RDPDR commitait donc sur un hook vert et ne
# voyait l'échec qu'en CI ; et la règle, prise au mot, laissait croire le hook
# exhaustif. La règle est corrigée en « trois portes obligatoires » ; le hook y
# est décrit pour ce qu'il est. Ce contrôle échoue contre la formulation
# d'origine.
#
# Deux invariants :
#   1. Les trois portes obligatoires (check.sh, ci.yml, .gitlab-ci.yml) lancent
#      bien `cargo test` pour test-rdp-server ET test-vnc-server.
#   2. Si le hook n'exécute pas ces serveurs, la règle de CONTRIBUTING.md ne
#      doit pas l'énumérer parmi les portes sur lesquelles toute crate hors
#      workspace « doit être branchée ». Sur-promettre le hook, c'est le défaut.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import pathlib
import re
import sys

SERVEURS = ("test-rdp-server", "test-vnc-server")
echecs = []

def lit(nom):
    return pathlib.Path(nom).read_text(encoding="utf-8")

# --- Invariant 1 : les trois portes obligatoires jouent les deux serveurs ----
# On cherche, dans chaque porte, un « cargo test » qui vise chaque serveur —
# soit nommé explicitement, soit via une boucle « for s in … test-rdp-server
# test-vnc-server … cargo test --manifest-path "$s/… » (GitLab et le bloc du
# hook Linux emploient cette forme). Il suffit que le nom du serveur et un
# `cargo test` cohabitent dans la porte, le serveur alimentant ce test.
portes = {
    "check.sh": lit("check.sh"),
    ".github/workflows/ci.yml": lit(".github/workflows/ci.yml"),
    ".gitlab-ci.yml": lit(".gitlab-ci.yml"),
}
for porte, texte in portes.items():
    if "cargo test" not in texte:
        echecs.append(f"{porte} : aucun `cargo test`")
        continue
    for serveur in SERVEURS:
        if serveur not in texte:
            echecs.append(
                f"{porte} : ne mentionne pas {serveur} — la porte obligatoire "
                f"ne joue plus ses tests"
            )

# --- Invariant 2 : le hook ne doit pas être sur-promis par la règle ----------
hook = pathlib.Path("scripts/hooks/pre-commit").read_text(encoding="utf-8")
# Le hook « joue » un serveur s'il lance ses tests. On exige la conjonction du
# nom du serveur et d'un `cargo test` : le seul fait de nommer le dossier ne
# suffit pas (le hook ne le nomme d'ailleurs pas aujourd'hui).
hook_joue_serveurs = "cargo test" in hook and all(s in hook for s in SERVEURS)

contributing = lit("CONTRIBUTING.md")
# La règle : la phrase impérative « … hors du workspace doit être branchée … »
# suivie de l'énumération des portes, jusqu'à la fin du bloc de citation (une
# ligne qui ne commence plus par « > »).
lignes = contributing.splitlines()
regle = None
for i, ligne in enumerate(lignes):
    if "hors du workspace doit être branchée" in ligne:
        bloc = []
        j = i
        while j < len(lignes) and lignes[j].lstrip().startswith(">"):
            bloc.append(lignes[j])
            j += 1
        regle = (i + 1, "\n".join(bloc))
        break

if regle is None:
    echecs.append(
        "CONTRIBUTING.md : règle « toute crate hors workspace doit être "
        "branchée … » introuvable (a-t-elle été renommée ?)"
    )
else:
    no_ligne, texte_regle = regle
    # On aplatit le bloc de citation (« > » et retours à la ligne retirés) puis
    # on n'inspecte QUE l'énumération des portes : le segment entre le tiret qui
    # l'introduit et la fin de sa phrase. Un point suivi d'une espace et d'une
    # majuscule borne la phrase — ni « check.sh » ni « ci.yml » ne s'y trompent
    # (leur point est suivi d'une minuscule). Ainsi une phrase ULTÉRIEURE qui
    # nomme le hook pour dire qu'il n'est PAS une porte obligatoire (« Le hook
    # reste une barrière rapide… ») n'est pas comptée comme une énumération.
    plat = " ".join(
        ligne.lstrip().lstrip(">").strip() for ligne in texte_regle.splitlines()
    )
    apres_tiret = re.search(r"—(.*)", plat)
    enumeration = ""
    if apres_tiret:
        zone = apres_tiret.group(1)
        borne = re.search(r"\.\s+[A-ZÀ-Ý]", zone)
        enumeration = zone[: borne.start()] if borne else zone
    nomme_le_hook = "hook" in enumeration
    if nomme_le_hook and not hook_joue_serveurs:
        echecs.append(
            f"CONTRIBUTING.md:{no_ligne} : la règle énumère le hook de "
            f"pré-commit parmi les portes où toute crate hors workspace doit "
            f"être branchée, alors que le hook ne joue pas les tests des "
            f"serveurs de test. Sortir le hook de la liste des portes "
            f"obligatoires (barrière rapide : workspace + processus RDP)."
        )

if echecs:
    for e in echecs:
        print(f"  ✗ {e}", file=sys.stderr)
    sys.exit(1)

print(
    "  ✓ trois portes obligatoires jouent les serveurs de test ; "
    "la règle ne sur-promet pas le hook"
)
PY
