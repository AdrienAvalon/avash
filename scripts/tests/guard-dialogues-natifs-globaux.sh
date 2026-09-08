#!/usr/bin/env bash
# Contrôle reproductible : la garde front (scripts/guard.sh) doit proscrire les
# dialogues natifs bloquants sous TOUTES leurs formes d'appel, y compris
# préfixées par un objet global (window., globalThis., self.), et pas seulement
# l'appel nu.
#
# Trouvé par l'audit du 8 septembre 2026 : les motifs de guard.sh étaient
# `(^|[^.a-zA-Z])(confirm|prompt)\(` et `(^|[^.a-zA-Z])alert\(`. La classe
# négative `[^.a-zA-Z]` exclut le point AVANT le nom, si bien que
# `window.confirm(`, `window.alert(`, `globalThis.prompt(` et `self.alert(`
# n'étaient PAS détectés — alors que ce sont exactement les dialogues natifs
# inopérants sous WebKitGTK/WRY que la garde veut interdire (confirm renvoie une
# Promise toujours vraie, prompt renvoie null, alert ne bloque pas). Un correctif
# écrivant `if (!window.confirm(t("supprimer ?"))) return;` passait garde,
# check.sh, le hook et les deux CI au vert, et sous Linux la suppression n'était
# jamais confirmée.
#
# Ce test copie la VRAIE guard.sh dans un bac à sable avec des fixtures et
# l'exécute de bout en bout : il exige qu'elle rougisse sur les formes préfixées
# et nues, et qu'elle reste verte sur les appels légitimes (askConfirm/notify) et
# les commentaires qui nomment ces dialogues. Contre les anciens motifs (avec le
# `.` dans la classe négative), les formes préfixées passaient inaperçues : le
# test rougissait.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

guard="$PWD/scripts/guard.sh"
echec=0

# Rejoue la vraie guard.sh dans un bac à sable qui imite la disposition du dépôt
# (scripts/guard.sh cd vers `dirname/..` puis grep `web/*.ts web/index.html`),
# avec un unique fichier web contenant $1. Renvoie le code de sortie de guard.sh.
joue_garde() { # contenu_du_fichier_web
  local bac fichier code
  bac="$(mktemp -d)"
  mkdir -p "$bac/scripts" "$bac/web"
  cp "$guard" "$bac/scripts/guard.sh"
  printf '%s\n' "$1" > "$bac/web/cas.ts"
  # index.html doit exister (motif du glob) : un fichier vide suffit.
  : > "$bac/web/index.html"
  ( cd "$bac" && bash scripts/guard.sh >/dev/null 2>&1 ) && code=0 || code=$?
  rm -rf "$bac"
  return "$code"
}

# Doit ROUGIR (code non nul) : dialogues natifs sous toutes leurs formes.
doit_rougir() { # description  contenu
  if joue_garde "$2"; then
    echo "  ✗ garde restée verte alors qu'elle devait proscrire : $1" >&2
    echo "      contenu : $2" >&2
    echec=1
  fi
}

# Doit RESTER VERTE (code 0) : appels légitimes et commentaires.
doit_verdir() { # description  contenu
  if ! joue_garde "$2"; then
    echo "  ✗ garde a rougi sur un cas légitime : $1" >&2
    echo "      contenu : $2" >&2
    echec=1
  fi
}

doit_rougir "window.confirm(" 'if (!window.confirm(t("supprimer ?"))) return;'
doit_rougir "window.alert("   'window.alert("échec");'
doit_rougir "globalThis.prompt(" 'const x = globalThis.prompt("nom");'
doit_rougir "self.alert("     'self.alert("coucou");'
doit_rougir "confirm( nu"     'if (!confirm("ok")) return;'
doit_rougir "prompt( nu"      'const n = prompt("valeur");'
doit_rougir "alert( nu"       'alert("boum");'

doit_verdir "askConfirm(" 'if (!(await askConfirm("supprimer ?"))) return;'
doit_verdir "askText("    'const n = await askText("nom");'
doit_verdir "notify("     'notify("échec");'
doit_verdir "commentaire // qui nomme alert" '// ne pas utiliser alert() ici'
doit_verdir "commentaire // qui nomme window.confirm" '// window.confirm() est inopérant sous WebKitGTK'

if [ "$echec" -ne 0 ]; then
  echo "✗ garde-dialogues-natifs-globaux : la garde front ne couvre pas toutes les formes." >&2
  exit 1
fi
echo "✓ garde-dialogues-natifs-globaux : formes préfixées et nues proscrites, appels légitimes épargnés"
