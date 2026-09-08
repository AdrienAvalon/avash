#!/usr/bin/env bash
# Contrôle reproductible : la section « Versions supportées » de SECURITY.md ne
# fige aucun numéro de version périmé face à la version courante du dépôt.
#
# Trouvé par l'audit du 8 septembre 2026 : le tableau annonçait « 0.6.x | Oui »
# / « < 0.6 | Non » et la phrase « la série 0.6.x, la dernière publiée » alors
# que Cargo.toml était en 0.9.2 — figé depuis la 0.6.2 (commit 6224f0a) malgré
# sept versions publiées. Un rapporteur de faille sur la 0.9.2 y lisait que sa
# version n'était pas dans une série supportée. Corrigé par une formulation sans
# numéro (« la dernière version publiée »), pour que la section ne redérive plus
# à chaque release ; ce contrôle veille à ce que si un numéro y réapparaît un
# jour, il colle à la version majeure.mineure courante.
#
# Portée : la section s'arrête au rappel historique (le bloc `>` qui cite 0.3.0,
# 0.6.1 et 0.6.2), lequel est un rappel daté légitime des correctifs passés, pas
# une déclaration de support — ces numéros ne doivent donc pas faire rougir.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echecs=0
security_md="SECURITY.md"

# Vérité : la version majeure.mineure du workspace.
version="$(grep -m1 '^version = ' Cargo.toml | sed -E 's/.*"([0-9]+\.[0-9]+).*/\1/')"
if [[ -z "$version" ]]; then
  echo "  ✗ impossible de lire la version du workspace dans Cargo.toml" >&2
  exit 1
fi

# La section « Versions supportées », arrêtée au rappel historique (`>`) ou au
# titre suivant.
section="$(awk '
  /^## Versions supportées/ { dans=1; next }
  dans && /^## / { exit }
  dans && /^>/ { exit }
  dans { print }
' "$security_md")"

if [[ -z "${section//[[:space:]]/}" ]]; then
  echo "  ✗ $security_md : section « Versions supportées » introuvable ou vide" >&2
  exit 1
fi

# Tout numéro majeur.mineur cité dans cette section doit coller à la version
# courante. Aucun numéro (formulation sans version) est le cas nominal attendu.
while read -r trouve; do
  [[ -z "$trouve" ]] && continue
  if [[ "$trouve" != "$version" ]]; then
    echo "  ✗ $security_md : la section « Versions supportées » cite la série $trouve alors que le dépôt est en $version" >&2
    echo "    → corriger le numéro, ou (préférable) formuler sans version (« la dernière version publiée »)" >&2
    echecs=1
  fi
done < <(grep -oE '[0-9]+\.[0-9]+' <<< "$section" | sort -u)

if [[ "$echecs" -ne 0 ]]; then
  exit 1
fi

echo "  ✓ $security_md : « Versions supportées » sans numéro périmé (dépôt en $version)"
