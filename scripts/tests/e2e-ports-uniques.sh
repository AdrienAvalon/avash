#!/usr/bin/env bash
# Contrôle reproductible : deux scénarios bout en bout ne démarrent jamais leur
# serveur de test sur le même port.
#
# Trouvé le 10 septembre 2026 sur la chaîne GitHub : `rdp-fichiers.spec.js` et
# `rdp-lecteur.spec.js` lançaient chacun un serveur RDP de test sur 33896, l'un
# juste après l'autre. Le `after` du premier envoie SIGTERM sans attendre la
# fin du processus ; le second serveur trouvait le port encore pris, sortait
# aussitôt, et son `waitForPort` expirait (« port 33896 pas prêt à temps »),
# quinze secondes ou trente, peu importe. Trois autres scénarios partageaient
# 33897 de la même façon et n'avaient pas encore perdu la course. Un port par
# scénario : la course n'existe plus, et ce contrôle interdit qu'elle revienne.
set -uo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

# Chaque déclaration `const PORT = 33896;` (ou RDP_PORT, VNC_PORT, PORT_X) d'un
# scénario est un serveur de test que ce scénario lance.
doublons="$(grep -HoE "const (PORT|RDP_PORT|VNC_PORT|PORT_[A-Z]+) *= *[0-9]+" e2e/specs/*.spec.js \
  | sed -E 's/^([^:]+):.*= *([0-9]+)$/\2 \1/' | sort | awk '{p[$1]=p[$1]" "$2; n[$1]++} END {for (k in n) if (n[k]>1) print k":"p[k]}')"

if [ -n "$doublons" ]; then
  echo "  ✗ des scénarios bout en bout partagent un port de serveur de test :" >&2
  sed 's/^/      /' <<<"$doublons" >&2
  echo "      donner à chacun son port : le serveur du scénario précédent tient encore le sien quand le suivant démarre" >&2
  exit 1
fi
echo "  ✓ e2e : un port de serveur de test par scénario"
