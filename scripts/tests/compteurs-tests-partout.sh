#!/usr/bin/env bash
# Contrôle reproductible : les compteurs de tests affichés au lecteur disent
# tous la même chose que le tableau de docs/qualite.md, qui fait foi.
#
# Trouvé par l'audit du 9 septembre 2026 : la vitrine (site/index.html et sa
# version anglaise) annonçait « 1233 tests » depuis des semaines, alors que
# docs/qualite.md en comptait 1590, puis 1629. Les README avaient pris du retard
# de la même façon à chaque vague de tests. Un garde existait pour la feuille
# de route (feuille-de-route-compteurs-coherents.sh) et un pour les
# sous-comptes de docs/qualite.md, aucun pour les quatre autres emplacements que
# CLAUDE.md demande pourtant de tenir : README.md, README.en.md, site/index.html,
# site/en/index.html. Ce contrôle relit les six.
#
# Ce qui est comparé, depuis le tableau de docs/qualite.md :
#   cœur, intégration, interface, processus RDP, serveurs de test, paquets
#   portés, front, bout en bout, et le total en tête de section.
# Les README agrègent « cœur + intégration » sur une ligne, la feuille de route
# additionne les cinq niveaux Rust : les sommes sont recalculées ici.
set -uo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# verifie <racine> : compare les six emplacements sous <racine> ; rend 1 au
# premier écart, en le nommant.
verifie() {
  local r="$1" q ecarts=0
  q="$r/docs/qualite.md"
  local total coeur integ iface rdp serveurs portes front e2e
  total="$(grep -oE '^\*\*[0-9]+ tests\*\*' "$q" | grep -oE '[0-9]+' | head -1)"
  cellule() { # <début de ligne du tableau> -> nombre de la 2e colonne
    grep -E "^\| $1 \|" "$q" | head -1 | awk -F'|' '{gsub(/ /,"",$3); print $3}'
  }
  coeur="$(cellule 'Cœur \(`crates/avash`\)')"
  integ="$(cellule 'Intégration')"
  iface="$(cellule 'Interface \(`crates/avash-ui`\)')"
  rdp="$(cellule 'Processus RDP')"
  serveurs="$(cellule 'Serveurs de test')"
  portes="$(cellule 'Paquets IronRDP et vnc-rs portés')"
  front="$(cellule 'Front \(Vitest\)')"
  e2e="$(cellule 'Bout en bout \(WebdriverIO\)')"
  for v in total coeur integ iface rdp serveurs portes front e2e; do
    if ! [[ "${!v}" =~ ^[0-9]+$ ]]; then
      echo "  ✗ docs/qualite.md : compteur « $v » illisible (« ${!v} »)" >&2
      return 1
    fi
  done
  local somme=$((coeur + integ + iface + rdp + serveurs + portes + front + e2e))
  if [ "$somme" -ne "$total" ]; then
    echo "  ✗ docs/qualite.md : le total annoncé ($total) n'est pas la somme du tableau ($somme)" >&2
    ecarts=1
  fi
  local rust=$((coeur + integ + iface + rdp + serveurs))

  attend() { # <fichier> <description> <motif grep -E>
    if ! grep -qE "$3" "$r/$1"; then
      echo "  ✗ $1 : $2 attendu « $3 »" >&2
      ecarts=1
    fi
  }
  attend README.md "badge du total" "tests-${total}%20verts"
  attend README.md "total en gras" "^\*\*${total} tests\*\* "
  attend README.md "cœur + intégration" "^\| Cœur Rust et intégration contre un vrai sshd \| $((coeur + integ)) \|"
  attend README.md "interface" "^\| Interface Tauri \| ${iface} \|"
  attend README.md "processus RDP" "^\| Processus RDP \| ${rdp} \|"
  attend README.md "serveurs de test" "^\| Serveurs de test \| ${serveurs} \|"
  attend README.md "paquets portés" "^\| Paquets IronRDP et vnc-rs portés \| ${portes} \|"
  attend README.md "front" "^\| Front \(Vitest\) \| ${front} \|"
  attend README.md "bout en bout" "^\| Bout en bout \(WebdriverIO\) \| ${e2e} \|"
  attend README.en.md "badge du total" "tests-${total}%20passing"
  attend README.en.md "total en gras" "^\*\*${total} tests\*\* "
  attend README.en.md "cœur + intégration" "^\| Rust core and integration against a real sshd \| $((coeur + integ)) \|"
  attend README.en.md "interface" "^\| Tauri interface \| ${iface} \|"
  attend README.en.md "processus RDP" "^\| RDP process \| ${rdp} \|"
  attend README.en.md "serveurs de test" "^\| Test servers \| ${serveurs} \|"
  attend README.en.md "paquets portés" "^\| Vendored IronRDP and vnc-rs crates \| ${portes} \|"
  attend README.en.md "front" "^\| Front \(Vitest\) \| ${front} \|"
  attend README.en.md "bout en bout" "^\| End to end \(WebdriverIO\) \| ${e2e} \|"
  attend docs/feuille-de-route.md "ligne Tests" \
    "^\| Tests \| ${rust} Rust \(${coeur} cœur, ${integ} intégration, ${iface} interface, ${rdp} processus RDP, ${serveurs} serveurs de test\) · ${portes} dans les paquets IronRDP et vnc-rs portés · ${front} front · ${e2e} scénarios"
  attend site/index.html "chiffre de la vitrine" "<b>${total}</b><span>tests, "
  attend site/en/index.html "chiffre de la vitrine anglaise" "<b>${total}</b><span>tests, "
  return "$ecarts"
}

echec=0

# --- 1. Le dépôt lui-même.
if ! verifie "$PWD"; then
  echo "  ✗ compteurs : un emplacement ne dit pas ce que docs/qualite.md compte (voir CLAUDE.md, « Tests : ce qu'on attend »)" >&2
  echec=1
fi

# --- 2. Contrôle négatif : une copie du dépôt dont la vitrine a pris du retard
# doit rougir, sinon ce script ne prouve rien.
bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT
mkdir -p "$bac/docs" "$bac/site/en"
for f in README.md README.en.md docs/qualite.md docs/feuille-de-route.md site/index.html site/en/index.html; do
  cp "$f" "$bac/$f"
done
if ! verifie "$bac" 2>/dev/null; then
  echo "  ✗ contrôle négatif : la copie conforme du dépôt a rougi (le contrôle lit mal un fichier)" >&2
  echec=1
fi
sed -i -E 's/<b>[0-9]+<\/b><span>tests, /<b>1233<\/b><span>tests, /' "$bac/site/index.html"
if verifie "$bac" 2>/dev/null; then
  echo "  ✗ contrôle négatif : une vitrine en retard (1233) est passée pour à jour" >&2
  echec=1
fi
cp README.md "$bac/README.md"
sed -i -E 's/^\| Interface Tauri \| [0-9]+ \|/| Interface Tauri | 1 |/' "$bac/README.md"
cp site/index.html "$bac/site/index.html"
if verifie "$bac" 2>/dev/null; then
  echo "  ✗ contrôle négatif : une ligne du tableau du README en retard est passée pour à jour" >&2
  echec=1
fi

if [ "$echec" -eq 0 ]; then
  echo "  ✓ compteurs : README, README.en, feuille de route et vitrine disent ce que docs/qualite.md compte"
fi
exit "$echec"
