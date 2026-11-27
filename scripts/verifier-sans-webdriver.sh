#!/usr/bin/env bash
# Vérifie qu'un binaire d'Avash ne contient pas le serveur WebDriver embarqué
# (fonctionnalité cargo `webdriver`, greffon tauri-plugin-wdio-webdriver).
#
#   scripts/verifier-sans-webdriver.sh <binaire>...
#   scripts/verifier-sans-webdriver.sh --doit-contenir <binaire>...
#
# Pourquoi : ce serveur écoute sur la boucle locale (127.0.0.1:4445) et donne à
# tout processus du poste le pilotage complet de l'application (terminaux SSH
# ouverts, frappes, mots de passe tapés). La règle « jamais dans un binaire
# publié » ne tenait qu'à l'absence de `--features webdriver` dans la commande
# de release.yml ; rien ne regardait le binaire lui-même (audit du 27 novembre
# 2026, SE-2). Le job `build` de release.yml appelle ce script sur chaque
# binaire avant d'envoyer les artefacts.
#
# Comment : on cherche dans le binaire des littéraux propres au greffon, lus
# dans ses sources (version épinglée =1.5.0 dans crates/avash-ui/Cargo.toml),
# et non des noms de fonctions, que l'éditeur de liens et `strip` effacent :
#   - `TAURI_WEBDRIVER_PORT`, la variable d'environnement que lit `init()`
#     (src/lib.rs, `PORT_ENV_VAR`), compilée sur toutes les plateformes ;
#   - `AlertStateManager lock poisoned`, le message de l'`expect` de
#     src/platform/alert_state.rs, module compilé lui aussi partout.
# Vérifié le 27 novembre 2026 : présents tous deux dans un `avash-ui` construit
# avec `--features webdriver`, absents tous deux sans. `strip = true` ne retire
# pas les chaînes de `.rodata`.
#
# `--doit-contenir` est le contrôle inverse, joué par les jobs bout en bout de
# ci.yml qui construisent avec `--features webdriver` : il exige TOUS les
# littéraux. Sans lui, une montée du greffon qui renommerait l'un d'eux rendrait
# la garde muette sans que personne ne le voie.
set -euo pipefail

LITTERAUX=(
  "TAURI_WEBDRIVER_PORT"
  "AlertStateManager lock poisoned"
)

usage() {
  echo "usage : $0 [--doit-contenir] <binaire>..." >&2
  exit 2
}

doit_contenir=0
if [ "${1:-}" = "--doit-contenir" ]; then
  doit_contenir=1
  shift
fi
[ "$#" -ge 1 ] || usage

# Recherche d'octets, sans locale : un binaire n'est pas de l'UTF-8 valide.
export LC_ALL=C

echec=0
for binaire in "$@"; do
  if [ ! -f "$binaire" ]; then
    echo "::error::$binaire : fichier absent, rien à vérifier." >&2
    echec=1
    continue
  fi
  presents=()
  absents=()
  for litteral in "${LITTERAUX[@]}"; do
    if grep -q -a -F -e "$litteral" -- "$binaire"; then
      presents+=("$litteral")
    else
      absents+=("$litteral")
    fi
  done
  if [ "$doit_contenir" -eq 1 ]; then
    if [ "${#absents[@]}" -ne 0 ]; then
      echo "::error::$binaire est construit avec --features webdriver mais ne contient pas : ${absents[*]}." \
           "Le greffon a changé ses littéraux : mettre à jour LITTERAUX dans $0, sinon la garde de la publication ne voit plus rien." >&2
      echec=1
    else
      echo "  ✓ $binaire contient le serveur WebDriver (${#presents[@]} littéraux) : la garde le reconnaît"
    fi
  else
    if [ "${#presents[@]}" -ne 0 ]; then
      echo "::error::$binaire contient le serveur WebDriver embarqué (${presents[*]}) : la fonctionnalité webdriver ne doit jamais entrer dans un binaire publié." >&2
      echec=1
    else
      echo "  ✓ $binaire sans serveur WebDriver embarqué"
    fi
  fi
done
exit "$echec"
