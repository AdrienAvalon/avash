#!/usr/bin/env bash
# Contrôle reproductible : la garde (scripts/guard.sh) doit proscrire, dans les
# sources Rust, les restes de mise au point et de contrôle négatif.
#
# Trouvé le 8 septembre 2026 : un contrôle négatif (correctif neutralisé pour
# vérifier que son test rougit) a été laissé en place dans `run_avec_agent` —
# `break false, // CONTROLE NEGATIF` à la place de `break true`. Format, clippy
# et tests unitaires du module passaient ; seul le `check.sh` complet, relancé
# à la main, l'a attrapé par un test d'intégration. Le marqueur, lui, est trivial
# à repérer, et c'est ce que ce test exige de la garde.
#
# On rejoue la VRAIE guard.sh dans un bac à sable qui imite la disposition du
# dépôt (web/ pour la partie front, et les répertoires Rust qu'elle scanne) avec
# une unique source Rust contenant le cas. Contre une garde qui ignorerait le
# Rust, tous les cas « doit rougir » resteraient verts : le test rougirait.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

guard="$PWD/scripts/guard.sh"
echec=0

joue_garde() { # contenu_de_la_source_rust
  local bac code
  bac="$(mktemp -d)"
  mkdir -p "$bac/scripts" "$bac/web" \
    "$bac/crates/avash/src" "$bac/crates/avash-ui/src" "$bac/rdp-sidecar/src" \
    "$bac/test-rdp-server/src" "$bac/test-vnc-server/src"
  cp "$guard" "$bac/scripts/guard.sh"
  # Partie front neutre : la garde doit y rester verte.
  : > "$bac/web/index.html"
  printf 'export const x = 1;\n' > "$bac/web/cas.ts"
  printf '%s\n' "$1" > "$bac/crates/avash/src/cas.rs"
  ( cd "$bac" && bash scripts/guard.sh >/dev/null 2>&1 ) && code=0 || code=$?
  rm -rf "$bac"
  return "$code"
}

doit_rougir() { # description  contenu
  if joue_garde "$2"; then
    echo "  ✗ garde restée verte alors qu'elle devait proscrire : $1" >&2
    echo "      contenu : $2" >&2
    echec=1
  fi
}

doit_verdir() { # description  contenu
  if ! joue_garde "$2"; then
    echo "  ✗ garde a rougi sur un cas légitime : $1" >&2
    echo "      contenu : $2" >&2
    echec=1
  fi
}

# Le cas réel du 8 septembre, en fin de ligne de code.
doit_rougir "contrôle négatif en fin de ligne" 'Some(russh::ChannelMsg::Failure) => break false, // CONTROLE NEGATIF'
doit_rougir "contrôle négatif sur sa ligne"    '// CONTROLE NEGATIF : neutralisé'
doit_rougir "contrôle négatif accentué"        '// CONTRÔLE NÉGATIF'
doit_rougir "contrôle négatif avec tiret bas"  'let _ = x; // CONTROLE_NEGATIF'
doit_rougir "dbg! oublié"                      'let y = dbg!(x + 1);'
doit_rougir "todo! oublié"                     'fn f() { todo!() }'

doit_verdir "code ordinaire"                   'fn f(a: u32) -> u32 { a + 1 }'
doit_verdir "commentaire en prose (minuscules)" '// un contrôle négatif a été fait à la main, puis remis'
doit_verdir "commentaire qui nomme dbg!"       '// ne pas laisser de dbg!() ici'
doit_verdir "commentaire qui nomme todo!"      '/* pas de todo!() dans une source commitée */'

if [ "$echec" -ne 0 ]; then
  echo "✗ guard-marqueurs-rust : la garde ne couvre pas les restes Rust." >&2
  exit 1
fi
echo "✓ guard-marqueurs-rust : contrôle négatif, dbg! et todo! proscrits dans le Rust, prose et code sains épargnés"
