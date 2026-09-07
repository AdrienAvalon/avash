#!/usr/bin/env bash
# Contrôle reproductible du job `fuzz` de .github/workflows/securite.yml.
#
# Trouvé par l'audit du 7 septembre 2026 : le job `fuzz` repartait des seules
# graines (`seeds/`, 1 à 7 fichiers par cible) à chaque exécution. `fuzz/corpus`
# est ignoré par git (`fuzz/.gitignore`) et `Swatinem/rust-cache` ne met en
# cache que `~/.cargo` et `target` : les entrées que la couverture découvre
# disparaissent à la fin du job, si bien que l'exploration recommençait de zéro
# à chaque poussée sur `main` et chaque lundi (localement le corpus config_ssh
# comptait déjà des milliers d'entrées, dont aucune n'a jamais tourné en CI).
#
# Ce test exige, sur le job réel, un pas `actions/cache` qui persiste le corpus
# entre exécutions :
#   - `path` couvre `fuzz/corpus` ;
#   - une `key` et des `restore-keys` (recherche par préfixe) pour retrouver le
#     corpus enrichi d'un run précédent même quand la clé exacte diffère ;
#   - le pas n'est pas restreint aux seules pull_request (le fuzz tourne sur
#     push/schedule, pas sur PR) ;
#   - l'action est épinglée sur un commit (règle du dépôt : `@<sha> # vX`).
# Contre le workflow d'origine (aucun cache du corpus) il échoue ; avec le pas
# `actions/cache` sur `fuzz/corpus` il passe.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

WORKFLOW=".github/workflows/securite.yml"

python3 - "$WORKFLOW" <<'PY'
import re, sys, yaml

workflow = yaml.safe_load(open(sys.argv[1], encoding="utf-8"))
job = workflow["jobs"]["fuzz"]


def exclut_pull_request(expr):
    """Vrai si la condition `if` restreint le pas aux seules pull_request.

    On ne modélise que les formes écrites dans ce workflow : une comparaison
    sur `github.event_name`. Toute autre forme est considérée non restrictive
    (le pas tourne aussi hors PR), ce qui est le comportement prudent ici.
    """
    if expr is None:
        return False
    e = str(expr).strip()
    m = re.fullmatch(r"\$\{\{(.*)\}\}", e, re.S)
    if m:
        e = m.group(1).strip()
    return bool(re.fullmatch(r"github\.event_name\s*==\s*'pull_request'", e))


def caches_corpus(etape):
    uses = etape.get("uses", "")
    if not uses.startswith("actions/cache@"):
        return False
    with_ = etape.get("with") or {}
    path = str(with_.get("path", ""))
    return "fuzz/corpus" in path


pas_cache = [e for e in job["steps"] if caches_corpus(e)]
if not pas_cache:
    print(
        "  ✗ le job `fuzz` ne persiste pas `fuzz/corpus` (aucun pas "
        "`actions/cache` sur ce chemin) : les entrées découvertes par la "
        "couverture sont jetées à la fin du job et l'exploration recommence "
        "de zéro à chaque poussée et chaque lundi",
        file=sys.stderr,
    )
    sys.exit(1)

etape = pas_cache[0]
with_ = etape.get("with") or {}

if not with_.get("restore-keys"):
    print(
        "  ✗ le cache de `fuzz/corpus` n'a pas de `restore-keys` : sans "
        "recherche par préfixe, une clé exacte manquée repart d'un corpus vide",
        file=sys.stderr,
    )
    sys.exit(1)

if not with_.get("key"):
    print("  ✗ le cache de `fuzz/corpus` n'a pas de `key`", file=sys.stderr)
    sys.exit(1)

if exclut_pull_request(etape.get("if")):
    print(
        "  ✗ le cache de `fuzz/corpus` est restreint aux pull_request, alors "
        "que le fuzz tourne sur push/schedule : le corpus n'est jamais persisté",
        file=sys.stderr,
    )
    sys.exit(1)

# Règle du dépôt : actions épinglées sur leur commit, version en commentaire.
if not re.fullmatch(r"actions/cache@[0-9a-f]{40}", etape["uses"]):
    print(
        f"  ✗ l'action de cache n'est pas épinglée sur un commit : {etape['uses']!r}",
        file=sys.stderr,
    )
    sys.exit(1)

print("  ✓ le job `fuzz` persiste `fuzz/corpus` entre exécutions (cache + restore-keys)")
PY
