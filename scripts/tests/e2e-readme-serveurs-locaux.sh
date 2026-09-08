#!/usr/bin/env bash
# Contrôle reproductible : la description des serveurs locaux d'e2e/README.md
# dit ce que fait le code, sur deux points où elle mentait.
#
# Trouvé par l'audit du 8 septembre 2026 : la section « Isolation » affirmait
# qu'`onPrepare` (wdio.conf.js) « démarre aussi un serveur RDP de test local
# (127.0.0.1:33899) pour rdp.spec.js ». Le code dit l'inverse : `onPrepare` ne
# monte que le sshd (wdio.conf.js « démarrés PAR CHAQUE spec RDP »), et c'est
# `rdp.spec.js` qui lance son propre serveur dans son `before`. Le README se
# contredisait d'ailleurs lui-même, plus bas : « chaque spec RDP démarre son
# propre serveur ». Et la ligne « En CI (E2E_NO_RDP=1)… » décrivait un état
# révolu : les jobs Linux (ci.yml, .gitlab-ci.yml) et Windows jouent la suite
# complète, serveurs compris ; seul le job macOS pose `E2E_NO_RDP`. Un
# contributeur cherchait le serveur 33899 dans `onPrepare`, un autre croyait sa
# nouvelle spec à serveur exclue de la CI sans se soucier de ses ports.
#
# Sources de vérité (le code), vérifiées ici avant de juger la doc :
#  - `onPrepare` de wdio.conf.js ne démarre AUCUN serveur RDP ;
#  - au moins une spec RDP appelle `startRdpServer` dans son propre corps ;
#  - les jobs e2e Linux/GitLab lancent `npm test` SANS `E2E_NO_RDP`, seul macOS
#    le pose.
# Le contrôle rougit contre les deux anciennes formulations du README.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

conf="e2e/wdio.conf.js"
readme="e2e/README.md"
ci=".github/workflows/ci.yml"
gitlab=".gitlab-ci.yml"

echecs=0

# --- Vérité du code : onPrepare ne démarre pas de serveur RDP ---------------
# Corps d'onPrepare : de « onPrepare:` jusqu'à la clé de config suivante.
corps_onprepare="$(awk '/onPrepare:/{p=1} p{print} p&&/^  [a-zA-Z]+:/&&!/onPrepare:/{if(NR>1)exit}' "$conf")"
if grep -q "startRdpServer" <<<"$corps_onprepare"; then
  echo "  ✗ $conf : onPrepare appelle startRdpServer — la doc et ce contrôle supposent l'inverse, revoir les deux" >&2
  echecs=1
fi
if ! grep -qE 'startRdpServer\(' e2e/specs/rdp.spec.js; then
  echo "  ✗ e2e/specs/rdp.spec.js n'appelle plus startRdpServer : la source de vérité a bougé" >&2
  echecs=1
fi

# --- Vérité de la CI : E2E_NO_RDP seulement sur macOS -----------------------
# Le job Linux (ci.yml) et GitLab lancent `npm test` sans poser E2E_NO_RDP.
if grep -qE 'E2E_NO_RDP' "$gitlab"; then
  echo "  ✗ $gitlab pose E2E_NO_RDP : la doc et ce contrôle supposent la suite complète en CI Linux" >&2
  echecs=1
fi

# --- Le README ne doit pas attribuer le serveur RDP à onPrepare -------------
# Ancienne formulation : « Il démarre aussi un serveur RDP … » dans la section
# « Isolation », dont le sujet est wdio.conf.js/onPrepare.
if grep -qiE 'démarre aussi un .*serveur RDP' "$readme"; then
  echo "  ✗ $readme attribue encore le serveur RDP à onPrepare (« démarre aussi un serveur RDP »)" >&2
  echo "    → le serveur RDP est démarré par chaque spec RDP ; onPrepare ne monte que le sshd" >&2
  echecs=1
fi

# --- Le README ne doit pas présenter E2E_NO_RDP comme l'état de la CI -------
if grep -qiE 'En CI \(.?E2E_NO_RDP' "$readme"; then
  echo "  ✗ $readme présente E2E_NO_RDP comme l'état de la CI, alors que seul macOS le pose" >&2
  echo "    → formuler « Sans serveur local (E2E_NO_RDP=1, job macOS, ou poste sans sshd)… »" >&2
  echecs=1
fi

if [[ "$echecs" -ne 0 ]]; then
  exit 1
fi

echo "  ✓ e2e/README.md : serveurs par spec et E2E_NO_RDP décrits conformément au code"
