#!/usr/bin/env bash
# Contrôle reproductible : dans check.sh, les deux artefacts non versionnés
# qu'exige le build.rs d'avash-ui — le front (`web/dist`, frontendDist) et le
# binaire du sidecar déposé dans `binaries/` (externalBin) — sont fabriqués
# AVANT la première compilation du workspace (`cargo … --workspace`), et le
# dépôt du sidecar n'est pas silencé par `|| true`.
#
# Trouvé par l'audit du 8 septembre 2026 : check.sh lançait sa section Rust
# (`cargo check/test/clippy --workspace`, qui compile avash-ui) EN TÊTE, mais ne
# construisait le front qu'à la fin et ne déposait le sidecar que dans la section
# « Build release » sautée par `--quick`. Sur un clone neuf, `tauri_build` panique
# (« resource path … doesn't exist » pour l'externalBin, puis « frontendDist …
# doesn't exist ») : quatre étapes rouges au premier passage, et `--quick` jamais
# vert sans les commandes manuelles de CONTRIBUTING — alors que ci.yml, lui,
# construit déjà front puis sidecar avant sa section Rust. Le `|| true` sur le
# `cp` du sidecar masquait de surcroît un échec de copie.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import re
import sys

lignes = open("check.sh", encoding="utf-8").read().splitlines()

def premiere(motif, *, ignorer_commentaires=True):
    """Numéro (1-indexé) de la première ligne de code correspondant au motif."""
    rx = re.compile(motif)
    for no, ligne in enumerate(lignes, 1):
        nu = ligne.strip()
        if ignorer_commentaires and nu.startswith("#"):
            continue
        if rx.search(ligne):
            return no
    return None

echecs = []

# La première compilation du workspace : c'est elle qui déclenche le build.rs
# d'avash-ui et donc l'exigence des deux artefacts.
rust = premiere(r"cargo (check|test|clippy).*--workspace")
if rust is None:
    echecs.append("aucune commande `cargo … --workspace` trouvée dans check.sh")

# Prérequis 1 : le build du front, qui produit web/dist.
front = premiere(r"vite build")
if front is None:
    echecs.append("aucun build du front (`vite build`) trouvé dans check.sh")

# Prérequis 2 : le dépôt du binaire du sidecar dans binaries/.
depot = premiere(r"binaries/avash-rdp")
if depot is None:
    echecs.append("aucun dépôt du sidecar (`binaries/avash-rdp`) trouvé dans check.sh")

if rust is not None:
    if front is not None and front > rust:
        echecs.append(
            f"le build du front (`vite build`, ligne {front}) est APRÈS la "
            f"section Rust (`--workspace`, ligne {rust}) : web/dist manque à la "
            f"compilation d'avash-ui sur un clone neuf"
        )
    if depot is not None and depot > rust:
        echecs.append(
            f"le dépôt du sidecar (`binaries/avash-rdp`, ligne {depot}) est APRÈS "
            f"la section Rust (`--workspace`, ligne {rust}) : l'externalBin manque "
            f"à la compilation d'avash-ui sur un clone neuf"
        )

# Le `cp` du sidecar ne doit pas être silencé : un échec de copie doit rougir.
# On teste toute ligne de code mentionnant `avash-rdp` (le `cible=` en
# `binaries/…` comme la ligne `cp target/release/avash-rdp …` qui dépose
# vraiment le binaire) : c'est cette dernière qui portait le `|| true` d'origine.
for no, ligne in enumerate(lignes, 1):
    if "avash-rdp" in ligne and not ligne.strip().startswith("#"):
        if "|| true" in ligne:
            echecs.append(
                f"ligne {no} : le dépôt du sidecar est silencé par `|| true` — "
                f"un échec de copie doit rougir"
            )

if echecs:
    for e in echecs:
        print(f"  ✗ {e}", file=sys.stderr)
    print(
        "  → construire web/dist et déposer le sidecar en tête de check.sh, "
        "avant la section Rust (comme ci.yml), sans `|| true`",
        file=sys.stderr,
    )
    sys.exit(1)

print(
    f"  ✓ prérequis (front dist ligne {front}, sidecar ligne {depot}) "
    f"avant la section Rust (ligne {rust}), dépôt du sidecar non silencé"
)
PY
