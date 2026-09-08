#!/usr/bin/env bash
# Contrôle reproductible : un argument de parc inconnu ne doit PAS rendre un
# verdict vert, ni dans conformite.sh, ni dans parc-rdp.sh.
#
# Trouvé par l'audit du 8 septembre 2026 : l'aiguillage du parc se faisait par
# trois lignes `[ "$quoi" = "xfce" ] || [ "$quoi" = "tous" ] && eprouver xfce`.
# Toute autre valeur (faute de frappe « xcfe », singulier « tout » au lieu de
# « tous », « ssh-only »…) ne déclenchait aucun contrôle : dans conformite.sh
# `echecs` restait à 0 et le script imprimait « ✓ Conformité : tout est vert. »
# avec le code 0 sans rien avoir éprouvé ; dans parc-rdp.sh, « ✓ parc prêt »
# sans démarrer un seul conteneur. check.sh transmet `${PARC:-xfce}` tel quel,
# donc `PARC=xcfe CONFORMITE_RDP=1 ./check.sh` comptait une étape verte creuse.
#
# Ce test rejoue les deux vrais scripts avec un argument inconnu et exige un
# code non nul et un message d'usage, jamais un verdict vert. Il rejoue aussi la
# garde de non-vacuité de conformite.sh (compteur de contrôles) en neutralisant
# l'aiguillage sur une copie du script, pour prouver qu'une régression future de
# l'aiguillage — argument pourtant valide mais aucun contrôle joué — rougit au
# lieu d'imprimer « tout est vert ». Contre les anciens scripts (sans le `case`
# ni la garde), chaque cas passait vert, code 0 : le test rougissait.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echec=0

# 1. conformite.sh avec un argument inconnu : le `case` sort AVANT tout contrôle
#    (aucun conteneur ni cargo requis), avec un code non nul et l'usage.
sortie="$(bash scripts/conformite.sh xcfe 2>&1)" && code=0 || code=$?
if [ "$code" -eq 0 ]; then
  echo "  ✗ conformite.sh xcfe a rendu le code 0 (argument inconnu accepté)" >&2
  echo "      sortie : ${sortie:-<vide>}" >&2
  echec=1
fi
if printf '%s' "$sortie" | grep -q "tout est vert"; then
  echo "  ✗ conformite.sh xcfe imprime « tout est vert » sans rien éprouver" >&2
  echec=1
fi
if ! printf '%s' "$sortie" | grep -q "usage"; then
  echo "  ✗ conformite.sh xcfe n'imprime pas de message d'usage" >&2
  echo "      sortie : ${sortie:-<vide>}" >&2
  echec=1
fi

# 2. parc-rdp.sh up avec un argument inconnu : le `case` sort AVANT `lancer`
#    (aucun moteur de conteneur requis), avec un code non nul et l'usage.
sortie="$(bash scripts/parc-rdp.sh up xcfe 2>&1)" && code=0 || code=$?
if [ "$code" -eq 0 ]; then
  echo "  ✗ parc-rdp.sh up xcfe a rendu le code 0 (argument inconnu accepté)" >&2
  echo "      sortie : ${sortie:-<vide>}" >&2
  echec=1
fi
if printf '%s' "$sortie" | grep -q "parc prêt"; then
  echo "  ✗ parc-rdp.sh up xcfe imprime « parc prêt » sans démarrer de conteneur" >&2
  echec=1
fi
if ! printf '%s' "$sortie" | grep -q "usage"; then
  echo "  ✗ parc-rdp.sh up xcfe n'imprime pas de message d'usage" >&2
  echo "      sortie : ${sortie:-<vide>}" >&2
  echec=1
fi

# 3. Garde de non-vacuité de conformite.sh : sur une copie dont l'aiguillage est
#    neutralisé (régression future simulée), un argument VALIDE ne joue aucun
#    contrôle ; la garde du compteur doit rougir au lieu d'imprimer « tout est
#    vert ». On ne touche qu'aux lignes d'aiguillage `[ "$quoi" … ] && eprouver…`.
banc="$(mktemp -d)"
trap 'rm -rf "$banc"' EXIT
sed -E '/^\[ "\$quoi"/ s/&& eprouver[a-z_]*.*/\&\& true/' scripts/conformite.sh >"$banc/conformite.sh"
if ! grep -q "controles" "$banc/conformite.sh"; then
  echo "  ✗ la garde de non-vacuité (compteur « controles ») est absente de conformite.sh" >&2
  echec=1
fi
sortie="$(bash "$banc/conformite.sh" tous 2>&1)" && code=0 || code=$?
if [ "$code" -eq 0 ]; then
  echo "  ✗ conformite.sh sans aucun contrôle joué rend le code 0 (garde absente)" >&2
  echo "      sortie : ${sortie:-<vide>}" >&2
  echec=1
fi
if printf '%s' "$sortie" | grep -q "tout est vert"; then
  echo "  ✗ conformite.sh imprime « tout est vert » alors qu'aucun contrôle n'a été joué" >&2
  echec=1
fi

if [ "$echec" -ne 0 ]; then
  echo "  → valider l'argument (case xfce|gnome|ssh|tous) et garder le compteur de contrôles" >&2
  exit 1
fi

echo "  ✓ argument inconnu refusé (conformite.sh, parc-rdp.sh) et verdict vert exigeant un contrôle joué"
