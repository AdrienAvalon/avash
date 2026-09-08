#!/usr/bin/env bash
# Contrôle reproductible : dans scripts/parc-rdp.sh, quand /proc/self/mountinfo
# ne porte pas l'identifiant du conteneur, le repli `moi="$(hostname)"` doit
# rester atteignable, au lieu de tuer le script en silence.
#
# Trouvé par l'audit du 8 septembre 2026 : `grep -o -m1 'containers/<id64>'`
# sort non nul quand il ne trouve rien (ou 2 si mountinfo est illisible). Sous
# `set -euo pipefail`, l'affectation nue `moi=$(grep … | cut …)` — sans préfixe
# local/export qui masquerait le code — prend le code du pipeline (pipefail) et
# fait sortir le script AVANT la ligne de repli `[ -n "$moi" ] || moi="$(hostname)"`.
# Sur un exécuteur dont /etc/hostname n'est pas monté depuis …/containers/<id64>/
# (runtime ou data-root atypique, montage personnalisé), `parc-rdp.sh up`
# s'arrêtait en silence, code 1 (stderr de grep avalé), au lieu de tenter le
# repli prévu par le commentaire du script (job 31269).
#
# Ce test extrait du vrai script les deux lignes concernées (l'affectation et
# son repli), pointe le grep sur un mountinfo sans identifiant, et les rejoue
# sous `set -euo pipefail`. Contre le script d'origine (affectation sans
# `|| true`), le pipeline échoue, `set -e` tue le sous-shell et rien n'est
# imprimé : le test rougit. Un second cas (mountinfo absent, grep sort en 2)
# vérifie que le même repli couvre aussi le fichier illisible.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

SCRIPT=scripts/parc-rdp.sh

# L'affectation réelle de l'identifiant, et sa ligne de repli. On les extrait
# pour rester ancré au vrai fichier.
ligne_affect="$(grep -F "moi=\$(grep -o -m1 'containers/" "$SCRIPT")" || {
  echo "  ✗ ligne d'affectation « moi=\$(grep … containers/… ) » introuvable dans $SCRIPT" >&2
  exit 1
}
ligne_repli="$(grep -F '[ -n "$moi" ] || moi="$(hostname)"' "$SCRIPT")" || {
  echo "  ✗ ligne de repli « [ -n \"\$moi\" ] || moi=\"\$(hostname)\" » introuvable dans $SCRIPT" >&2
  exit 1
}

banc="$(mktemp -d)"
trap 'rm -rf "$banc"' EXIT

# Rejoue les deux vraies lignes avec le grep pointé sur $1 (le mountinfo du cas
# éprouvé), puis imprime `moi` : s'il est vide ou si le script est mort avant,
# le repli n'a pas joué.
rejouer() { # chemin_mountinfo
  local affect
  affect="$(printf '%s\n' "$ligne_affect" | sed "s#/proc/self/mountinfo#$1#")"
  bash -c '
    set -euo pipefail
    '"$affect"'
    '"$ligne_repli"'
    printf "moi=%s\n" "$moi"
  ' 2>/dev/null
}

echec=0

# Cas 1 : mountinfo présent mais sans « containers/<id64> » (runtime atypique).
#   grep sort en 1, le pipeline aussi ; le repli doit rattraper.
printf '36 35 0:32 / /proc rw shared:14 - proc proc rw\n' >"$banc/mountinfo"
sortie="$(rejouer "$banc/mountinfo")" && code=0 || code=$?
if [ "$code" -ne 0 ]; then
  echo "  ✗ mountinfo sans identifiant : le script est mort (code $code) avant le repli hostname" >&2
  echec=1
elif ! printf '%s' "$sortie" | grep -qE 'moi=.+'; then
  echo "  ✗ mountinfo sans identifiant : « moi » est resté vide, le repli hostname n'a pas joué" >&2
  echo "      sortie : ${sortie:-<vide>}" >&2
  echec=1
fi

# Cas 2 : mountinfo absent/illisible ; grep sort en 2. Même repli attendu.
sortie="$(rejouer "$banc/inexistant")" && code=0 || code=$?
if [ "$code" -ne 0 ]; then
  echo "  ✗ mountinfo illisible : le script est mort (code $code) avant le repli hostname" >&2
  echec=1
elif ! printf '%s' "$sortie" | grep -qE 'moi=.+'; then
  echo "  ✗ mountinfo illisible : « moi » est resté vide, le repli hostname n'a pas joué" >&2
  echo "      sortie : ${sortie:-<vide>}" >&2
  echec=1
fi

if [ "$echec" -ne 0 ]; then
  echo "  → ajouter '|| true' à la substitution grep pour que le repli hostname s'exécute" >&2
  exit 1
fi

echo "  ✓ mountinfo sans identifiant ou illisible : le repli hostname reste atteignable"
