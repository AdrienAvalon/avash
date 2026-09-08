#!/usr/bin/env bash
# Contrôle reproductible : dans scripts/captures-readme.sh, quand aucun mot de
# passe RDP n'est dans le trousseau, le message diagnostique « aucun mot de
# passe dans le trousseau » doit s'afficher (et le script sortir 1), au lieu de
# mourir en silence.
#
# Trouvé par l'audit du 8 septembre 2026 : `secret-tool lookup` sort non nul
# quand aucun secret ne correspond (man secret-tool : « On success 0 is
# returned, a non-zero failure code otherwise »). Sous `set -euo pipefail`,
# l'affectation `CAPTURES_RDP_MDP="$(secret-tool lookup …)"` — une affectation
# nue, sans préfixe local/export qui masquerait le code — fait sortir le script
# AVANT la ligne `[ -n … ] || { echo … ; exit 1; }` censée dire quoi enregistrer
# dans le trousseau. L'utilisateur voyait un arrêt muet, code 1, sans savoir
# s'il s'agissait du trousseau, de xvfb ou du binaire manquant.
#
# Ce test extrait du vrai script les deux lignes concernées (l'affectation et sa
# garde), les rejoue sous `set -euo pipefail` avec un faux `secret-tool` qui
# échoue (aucun secret), et exige que le message diagnostique soit imprimé.
# Contre le script d'origine (affectation sans `|| true`), la substitution
# échoue, `set -e` tue le sous-shell et rien n'est imprimé : le test rougit.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

SCRIPT=scripts/captures-readme.sh

# L'affectation réelle du mot de passe depuis le trousseau, et la garde qui
# émet le message. On les extrait pour rester ancré au vrai fichier.
ligne_mdp="$(grep -E 'CAPTURES_RDP_MDP="\$\(secret-tool lookup' "$SCRIPT")" || {
  echo "  ✗ ligne d'affectation CAPTURES_RDP_MDP introuvable dans $SCRIPT" >&2
  exit 1
}
ligne_garde="$(grep -E '\[ -n "\$CAPTURES_RDP_MDP" \]' "$SCRIPT")" || {
  echo "  ✗ garde [ -n \"\$CAPTURES_RDP_MDP\" ] introuvable dans $SCRIPT" >&2
  exit 1
}

# Faux secret-tool qui échoue (aucun secret dans le trousseau), sur PATH.
banc="$(mktemp -d)"
trap 'rm -rf "$banc"' EXIT
cat >"$banc/secret-tool" <<'STUB'
#!/usr/bin/env bash
exit 1
STUB
chmod +x "$banc/secret-tool"

# Variables dont dépendent les deux lignes (cf. le bloc `if [ -n "${1:-}" ]`).
sortie="$(
  PATH="$banc:$PATH" bash -c '
    set -euo pipefail
    CAPTURES_RDP_UTILISATEUR=Administrateur
    set -- dc01.exemple
    '"$ligne_mdp"'
    export CAPTURES_RDP_MDP
    '"$ligne_garde"'
  ' 2>&1
)" && code=0 || code=$?

if [ "$code" -ne 1 ]; then
  echo "  ✗ code de sortie attendu 1 (garde), obtenu $code" >&2
  echo "      sortie : ${sortie:-<vide>}" >&2
  exit 1
fi
if ! printf '%s' "$sortie" | grep -q "aucun mot de passe dans le trousseau"; then
  echo "  ✗ le message diagnostique du trousseau n'a pas été imprimé (arrêt muet)" >&2
  echo "      sortie : ${sortie:-<vide>}" >&2
  echo "  → ajouter '|| true' à la substitution secret-tool pour que la garde s'exécute" >&2
  exit 1
fi

echo "  ✓ trousseau vide : le message diagnostique s'affiche et le script sort 1"
