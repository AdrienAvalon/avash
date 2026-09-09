#!/usr/bin/env bash
# Empreinte de l'arbre de travail tel qu'il serait commité : HEAD, différences
# des fichiers suivis, contenu des fichiers non suivis (hors ignorés).
#
# Écrite par check.sh dans .git/avash-temoin-check quand il est vert, relue par
# le hook de pré-commit : si l'empreinte n'a pas bougé, l'arbre est exactement
# celui que check.sh vient de valider et le hook n'a rien à rejouer. Le moindre
# octet changé, fichier ajouté ou HEAD déplacé donne une autre empreinte.
# Trouvé par l'audit du 9 septembre 2026 : la porte était lente deux fois.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
{
  git rev-parse HEAD
  git diff --no-ext-diff --binary HEAD
  git ls-files --others --exclude-standard -z | LC_ALL=C sort -z | xargs -0 -r sha256sum
} | sha256sum | cut -d' ' -f1
