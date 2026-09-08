#!/usr/bin/env bash
# Contrôle reproductible : les deux tableaux chiffrés de docs/feuille-de-route.md
# — « Où nous en sommes » (le relevé) et « Comment savoir si l'on progresse »
# (les mesures à relever à chaque version) — s'accordent entre eux, avec le
# décompte réel des `it(` de e2e/specs et avec le relevé de couverture de
# docs/qualite.md.
#
# Trouvé par l'audit du 8 septembre 2026 : le tableau « Comment savoir » affichait
# encore « Scénarios bout en bout | 69 » (figé au commit du 05/09) et
# « Couverture des tests | 75 % … 66 % » (relevé d'avant la mesure unitaires +
# bout en bout), quand le même fichier donnait plus haut 71 scénarios et
# 84 %/81 %. Le lecteur qui suit la consigne du document (« relever à chaque
# version ») trouvait deux valeurs pour le même indicateur dans le même fichier.
# La source de vérité des scénarios est le décompte des `it(` ; celle de la
# couverture est docs/qualite.md. Ce contrôle prend ces deux sources et rougit
# tant que les deux tableaux ne les citent pas.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

feuille="docs/feuille-de-route.md"
qualite="docs/qualite.md"
echecs=0

# Vérité 1 : décompte réel des scénarios bout en bout (une déclaration `it(` par
# ligne, en tête de ligne), source de vérité partagée avec compteur-scenarios-e2e.sh.
reel_scenarios="$(grep -hcE '^\s*it\(' e2e/specs/*.spec.js | paste -sd+ - | { read -r somme; echo $((somme)); })"

# Vérité 2 : parts de couverture (entier) du relevé de docs/qualite.md.
# Espace de travail (cœur et interface) puis processus RDP.
couv_workspace="$(grep -E '^\| Espace de travail' "$qualite" | grep -oE '\*\*[0-9]+' | head -1 | tr -dc '0-9')"
couv_rdp="$(grep -E '^\| Processus RDP' "$qualite" | grep -oE '\*\*[0-9]+' | head -1 | tr -dc '0-9')"
if [[ -z "$couv_workspace" || -z "$couv_rdp" ]]; then
  echo "  ✗ $qualite : parts de couverture introuvables (espace de travail / processus RDP)" >&2
  exit 1
fi

# Tableau « Où nous en sommes » : ligne « … NN scénarios bout en bout … ».
ou_scenarios="$(grep -oE '[0-9]+ scénarios bout en bout' "$feuille" | grep -oE '^[0-9]+' | head -1 || true)"
if [[ "$ou_scenarios" != "$reel_scenarios" ]]; then
  echo "  ✗ $feuille (« Où nous en sommes ») annonce ${ou_scenarios:-?} scénarios, la suite en compte $reel_scenarios" >&2
  echecs=1
fi

# Tableau « Comment savoir » : ligne « Scénarios bout en bout | NN | … ».
cs_scenarios="$(grep -E '^\| Scénarios bout en bout ' "$feuille" | grep -oE '\| [0-9]+ ' | grep -oE '[0-9]+' | head -1 || true)"
if [[ "$cs_scenarios" != "$reel_scenarios" ]]; then
  echo "  ✗ $feuille (« Comment savoir ») annonce ${cs_scenarios:-?} scénarios, la suite en compte $reel_scenarios" >&2
  echecs=1
fi

# Tableau « Comment savoir » : ligne « Couverture des tests | … » doit citer les
# parts de qualite.md et non les anciennes.
cs_couv="$(grep -E '^\| Couverture des tests ' "$feuille" || true)"
if [[ -z "$cs_couv" ]]; then
  echo "  ✗ $feuille : ligne « Couverture des tests » introuvable dans « Comment savoir »" >&2
  echecs=1
else
  if ! grep -qE "\b$couv_workspace ?%" <<< "$cs_couv"; then
    echo "  ✗ $feuille (« Comment savoir ») : couverture ne cite pas $couv_workspace % (relevé qualite.md)" >&2
    echecs=1
  fi
  if ! grep -qE "\b$couv_rdp ?%" <<< "$cs_couv"; then
    echo "  ✗ $feuille (« Comment savoir ») : couverture ne cite pas $couv_rdp % du processus RDP (relevé qualite.md)" >&2
    echecs=1
  fi
fi

if [[ "$echecs" -ne 0 ]]; then
  echo "  → accorder les deux tableaux de $feuille au décompte des \`it(\` ($reel_scenarios) et au relevé de $qualite ($couv_workspace % / $couv_rdp %)" >&2
  exit 1
fi

echo "  ✓ $feuille : tableaux accordés ($reel_scenarios scénarios, $couv_workspace % / $couv_rdp % de couverture)"
