#!/usr/bin/env bash
# Contrôle reproductible : la section « 2.3 Import » de docs/feuille-de-route.md
# décrit bien ce que le code reprend, en particulier les bureaux RDP MobaXterm.
#
# Trouvé par l'audit du 8 septembre 2026 : la feuille de route affirmait « Seules
# les sessions SSH sont reprises ; les autres protocoles sont comptés et dits »,
# état antérieur au support des bureaux RDP. Le code, lui, lit les signets `#91`
# de MobaXterm (crates/avash/src/import.rs : `BureauImporte`, parse `#91#`) et les
# écrit dans le magasin rdphost (crates/avash-ui/src/commands/import.rs :
# `upsert_host_in(hosts_path(), …)`), ce que le README annonce déjà. Un lecteur
# de la feuille de route croyait donc l'import RDP absent. Ce contrôle prend le
# code pour vérité : si les bureaux RDP y sont repris, la section 2.3 ne doit pas
# les dire ignorés et doit les mentionner (`#91`).
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echecs=0
feuille="docs/feuille-de-route.md"
import_coeur="crates/avash/src/import.rs"

# Vérité : le cœur reprend-il les bureaux RDP MobaXterm ? (type dédié + parse #91)
if ! grep -q 'struct BureauImporte' "$import_coeur" || ! grep -q '#91#' "$import_coeur"; then
  echo "  ✓ $import_coeur ne reprend plus les bureaux RDP : rien à exiger de la feuille de route" >&2
  exit 0
fi

# Section « 2.3 Import », arrêtée au titre de niveau suivant.
section="$(awk '
  /^### 2\.3 / { dans=1; next }
  dans && /^### / { exit }
  dans { print }
' "$feuille")"

if [[ -z "${section//[[:space:]]/}" ]]; then
  echo "  ✗ $feuille : section « 2.3 Import » introuvable ou vide" >&2
  exit 1
fi

# La formulation périmée « Seules les sessions SSH sont reprises » nie l'import RDP.
if grep -qi 'Seules les sessions SSH sont reprises' <<< "$section"; then
  echo "  ✗ $feuille (2.3) : « Seules les sessions SSH sont reprises » alors que le code reprend les bureaux RDP MobaXterm" >&2
  echecs=1
fi

# La section doit citer les bureaux RDP et leur marqueur #91.
if ! grep -qi 'bureaux RDP' <<< "$section" || ! grep -q '#91' <<< "$section"; then
  echo "  ✗ $feuille (2.3) : l'import des bureaux RDP MobaXterm (\`#91\`) n'est pas mentionné" >&2
  echecs=1
fi

if [[ "$echecs" -ne 0 ]]; then
  exit 1
fi

echo "  ✓ $feuille (2.3) : import RDP MobaXterm décrit conformément au code"
