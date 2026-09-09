#!/usr/bin/env bash
# Contrôle reproductible : la ligne « Serveurs de test » du tableau des
# compteurs de docs/qualite.md dit vrai jusque dans son détail : le total
# affiché, les deux sous-comptes cités entre parenthèses (serveur VNC et côté
# serveur RDPDR) et le décompte réel des `#[test]` / `#[tokio::test]` des deux
# fichiers concernés s'accordent, et les sous-comptes s'additionnent au total.
#
# Trouvé par l'audit du 9 septembre 2026 : le commit ce6b2bc (la vague des
# constats bas de l'audit précédent) avait ajouté deux tests au serveur VNC de
# test et un au côté serveur RDPDR, porté le total de la ligne de 29 à 32, mais
# laissé le texte parenthétique citer « (2) » et « (27) ». Le lecteur qui suit
# l'addition annoncée obtenait 29 pour un total affiché de 32, sans savoir d'où
# venaient les trois tests manquants. Aucune garde ne relisait cette ligne :
# feuille-de-route-compteurs-coherents.sh et compteur-scenarios-e2e.sh ne
# couvrent que les scénarios bout en bout et les parts de couverture. Ce
# contrôle prend le code des deux crates pour vérité et rougit tant que la
# cellule ne le cite pas exactement.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

qualite="docs/qualite.md"
source_vnc="test-vnc-server/src/main.rs"
source_rdpdr="test-rdp-server/src/rdpdr/tests.rs"
echecs=0

for fichier in "$qualite" "$source_vnc" "$source_rdpdr"; do
  if [[ ! -f "$fichier" ]]; then
    echo "  ✗ $fichier introuvable : la garde ne peut plus prendre le code pour vérité" >&2
    exit 1
  fi
done

# Vérité : un attribut de test par ligne, en tête de ligne, dans les deux
# fichiers que la cellule nomme.
reel_vnc="$(grep -cE '^[[:space:]]*#\[(tokio::)?test\]' "$source_vnc")"
reel_rdpdr="$(grep -cE '^[[:space:]]*#\[(tokio::)?test\]' "$source_rdpdr")"
reel_total=$((reel_vnc + reel_rdpdr))

ligne="$(grep -E '^\| Serveurs de test ' "$qualite" || true)"
if [[ -z "$ligne" ]]; then
  echo "  ✗ $qualite : ligne « Serveurs de test » introuvable dans le tableau des compteurs" >&2
  exit 1
fi

# Total : deuxième colonne du tableau.
dit_total="$(awk -F'|' '{gsub(/[^0-9]/, "", $3); print $3}' <<< "$ligne")"
# Sous-compte du serveur VNC : « serveur VNC (N) ».
dit_vnc="$(grep -oE 'serveur VNC \([0-9]+\)' <<< "$ligne" | grep -oE '[0-9]+' | head -1 || true)"
# Sous-compte RDPDR : « `test-rdp-server/src/rdpdr/`, N) ».
dit_rdpdr="$(grep -oE 'test-rdp-server/src/rdpdr/`, [0-9]+\)' <<< "$ligne" | grep -oE '[0-9]+' | head -1 || true)"

if [[ -z "$dit_vnc" || -z "$dit_rdpdr" ]]; then
  echo "  ✗ $qualite : la ligne « Serveurs de test » ne cite plus ses deux sous-comptes sous la forme attendue" >&2
  echo "    → attendu « serveur VNC (N) » et « \`test-rdp-server/src/rdpdr/\`, N) »" >&2
  exit 1
fi

if [[ "$dit_vnc" != "$reel_vnc" ]]; then
  echo "  ✗ $qualite annonce $dit_vnc tests pour le serveur VNC, $source_vnc en compte $reel_vnc" >&2
  echecs=1
fi
if [[ "$dit_rdpdr" != "$reel_rdpdr" ]]; then
  echo "  ✗ $qualite annonce $dit_rdpdr tests côté serveur RDPDR, $source_rdpdr en compte $reel_rdpdr" >&2
  echecs=1
fi
if [[ "$dit_total" != "$reel_total" ]]; then
  echo "  ✗ $qualite annonce $dit_total tests de serveurs de test, les deux fichiers en comptent $reel_total" >&2
  echecs=1
fi
# L'incohérence interne visée par l'audit : l'addition annoncée au lecteur ne
# retombait pas sur le total affiché dans la même cellule.
if [[ $((dit_vnc + dit_rdpdr)) != "$dit_total" ]]; then
  echo "  ✗ $qualite : $dit_vnc + $dit_rdpdr = $((dit_vnc + dit_rdpdr)), mais la même ligne affiche $dit_total" >&2
  echecs=1
fi

if [[ "$echecs" -ne 0 ]]; then
  echo "  → accorder la ligne « Serveurs de test » de $qualite au code : $reel_total au total, serveur VNC ($reel_vnc), RDPDR ($reel_rdpdr)" >&2
  exit 1
fi

echo "  ✓ $qualite : ligne « Serveurs de test » exacte ($reel_total = VNC $reel_vnc + RDPDR $reel_rdpdr)"
