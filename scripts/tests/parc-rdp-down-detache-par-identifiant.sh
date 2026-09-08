#!/usr/bin/env bash
# Contrôle reproductible : dans scripts/parc-rdp.sh, la commande `down` doit
# détacher le conteneur du job par son IDENTIFIANT (la détection mountinfo, repli
# hostname, partagée avec `raccorder`), pas par un `$(hostname)` nu.
#
# Trouvé par l'audit du 8 septembre 2026 : `down` faisait
#   $MOTEUR network disconnect "$PARC_RESEAU" "$(hostname)"
# alors que `raccorder` documente (et l'a corrigé pour le connect) que sur
# l'exécuteur GitLab le nom d'hôte n'est PAS l'identifiant du conteneur. Le
# disconnect échouait donc exactement comme le connect avant lui — mais l'erreur
# était avalée (`|| true`), l'endpoint du job restait attaché au réseau, et le
# `docker network rm avash-parc` suivant échouait à son tour (« has active
# endpoints »), avalé lui aussi : le réseau avash-parc n'était jamais supprimé et
# survivait à chaque pipeline de conformité.
#
# Le test rejoue la vraie ligne de disconnect avec un mountinfo qui porte un
# identifiant connu et un `hostname` qui rend AUTRE chose, et capture l'argument
# passé à `network disconnect`. Contre le script d'origine (`$(hostname)`),
# l'argument est le faux nom d'hôte : le test rougit. Après correctif
# (`$(identifiant_conteneur)`), c'est l'identifiant du mountinfo qui est détaché.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

SCRIPT=scripts/parc-rdp.sh

# On extrait du vrai fichier la définition d'identifiant_conteneur() et la vraie
# ligne de disconnect de `down`, pour rester ancré au script livré.
if ! grep -q '^identifiant_conteneur() {' "$SCRIPT"; then
  echo "  ✗ fonction identifiant_conteneur() absente de $SCRIPT" >&2
  echo "  → extraire la détection de l'identifiant et l'employer dans raccorder ET down" >&2
  exit 1
fi
def_fonction="$(sed -n '/^identifiant_conteneur() {/,/^}/p' "$SCRIPT")"

ligne_disc="$(grep -F 'network disconnect "$PARC_RESEAU"' "$SCRIPT")" || {
  echo "  ✗ ligne « network disconnect \"\$PARC_RESEAU\" … » introuvable dans $SCRIPT" >&2
  exit 1
}

banc="$(mktemp -d)"
trap 'rm -rf "$banc"' EXIT

# mountinfo qui porte un identifiant connu (64 hex) et un hostname distinct.
id="0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
printf '42 41 0:36 /containers/%s/hostname /etc/hostname rw - overlay overlay rw\n' "$id" >"$banc/mountinfo"

# On rejoue la vraie ligne de disconnect en remplaçant :
#  - le grep sur /proc/self/mountinfo par notre banc (dans la fonction extraite) ;
#  - `hostname` par un faux qui rend un nom différent de l'identifiant ;
#  - `$MOTEUR network disconnect "$PARC_RESEAU"` par un echo qui capture la cible.
def_banc="$(printf '%s\n' "$def_fonction" | sed "s#/proc/self/mountinfo#$banc/mountinfo#")"
capture="$(printf '%s\n' "$ligne_disc" \
  | sed 's#\[ -f /\.dockerenv \] &&##' \
  | sed 's#>/dev/null 2>&1 || true##' \
  | sed 's#\$MOTEUR network disconnect "\$PARC_RESEAU"#printf "CIBLE=%s\\n"#')"

sortie="$(bash -c '
  set -euo pipefail
  hostname() { echo faux-nom-hote-du-job; }
  PARC_RESEAU=avash-parc
  '"$def_banc"'
  '"$capture"'
')"

cible="$(printf '%s\n' "$sortie" | sed -n 's/^CIBLE=//p')"

if [ "$cible" = "faux-nom-hote-du-job" ]; then
  echo "  ✗ down détache par le nom d'hôte ($cible), pas par l'identifiant du conteneur" >&2
  echo "  → utiliser \$(identifiant_conteneur) dans le network disconnect de down" >&2
  exit 1
fi
if [ "$cible" != "$id" ]; then
  echo "  ✗ down détache « $cible », attendu l'identifiant du conteneur ($id)" >&2
  exit 1
fi

echo "  ✓ down détache le conteneur du job par son identifiant (mountinfo, comme raccorder)"
