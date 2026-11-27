#!/usr/bin/env bash
# Contrôle reproductible : chaque commande `gh attestation verify` donnée aux
# utilisateurs impose le workflow signataire (`--signer-workflow` sur
# release.yml), pas seulement le dépôt.
#
# Trouvé par l'audit du 27 novembre 2026 (SE-3) : la commande documentée était
# `gh attestation verify <fichier> --repo AdrienAvalon/avash`, et le README
# affirmait qu'elle prouvait « ce commit, produit par notre chaîne ». Avec
# `--repo` seul, gh n'impose que le dépôt du certificat Sigstore ; cinq jobs du
# dépôt peuvent obtenir un jeton OIDC, et la vérification hors ligne
# (`--bundle`) acceptait un lot signé par n'importe lequel. Une commande de
# provenance (sans `--predicate-type`) impose aussi le tag (`--source-ref`) et
# refuse les exécuteurs auto-hébergés.
#
# Fichiers lus : les deux README, SECURITY.md, RELEASE.md, release.yml
# (commentaires et notes de la release compris) et la vitrine (français et
# anglais).
set -euo pipefail
RACINE="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
cd "$RACINE"

python3 - <<'PY'
import re
import sys

FICHIERS = ["README.md", "README.en.md", "SECURITY.md", "RELEASE.md", ".github/workflows/release.yml",
            "site/index.html", "site/en/index.html"]
SIGNATAIRE = "--signer-workflow AdrienAvalon/avash/.github/workflows/release.yml"
SIGNATAIRE_CI = "--signer-workflow ${GITHUB_REPOSITORY}/.github/workflows/release.yml"

echecs, vues = [], 0
for chemin in FICHIERS:
    texte = open(chemin, encoding="utf-8").read()
    # Lignes jointes : continuations `\`, y compris derrière un `#` de YAML.
    lignes = texte.splitlines()
    i = 0
    while i < len(lignes):
        n = i + 1
        ligne = lignes[i]
        while ligne.rstrip().endswith("\\") and i + 1 < len(lignes):
            i += 1
            ligne = ligne.rstrip()[:-1] + " " + re.sub(r"^\s*#?\s*", "", lignes[i])
        i += 1
        # Une phrase peut nommer la commande sans la donner (« par `gh
        # attestation verify --bundle` ») : on ne juge que les commandes
        # complètes, celles qui désignent le dépôt.
        for m in re.finditer(r"gh attestation verify\b[^`\n]*?--repo\b[^`\n]*", ligne):
            cmd = m.group(0)
            vues += 1
            if SIGNATAIRE not in cmd and SIGNATAIRE_CI not in cmd:
                echecs.append(f"{chemin}:{n} : sans --signer-workflow sur release.yml : {cmd[:100]}")
            if "--predicate-type" not in cmd:
                if "--source-ref refs/tags/" not in cmd:
                    echecs.append(f"{chemin}:{n} : provenance sans --source-ref refs/tags/… : {cmd[:100]}")
                if "--deny-self-hosted-runners" not in cmd:
                    echecs.append(f"{chemin}:{n} : provenance sans --deny-self-hosted-runners : {cmd[:100]}")

if vues == 0:
    echecs.append("aucune commande `gh attestation verify … --repo` trouvée : la documentation de la vérification a disparu")
if echecs:
    for e in echecs:
        print(f"  ✗ {e}", file=sys.stderr)
    sys.exit(1)
print(f"  ✓ {vues} commandes de vérification, toutes avec le workflow signataire imposé")
PY
