#!/usr/bin/env bash
# Contrôle reproductible : les droits posés par le harnais E2E sur le fichier
# de clés d'administrateur du sshd Windows sont exprimés par SID, jamais par
# nom de compte.
#
# Trouvé le 11 septembre 2026 sur un Windows 11 en français : le harnais
# faisait `icacls … /grant 'Administrators:F'`, or le groupe s'y appelle
# « Administrateurs ». icacls répondait « le mappage entre les noms de compte
# et les ID de sécurité n'a pas été effectué », la préparation du sshd
# échouait et la suite s'arrêtait avant le premier scénario. Les exécuteurs
# GitHub sont en anglais : ils ne pouvaient pas le voir. Les SID, eux, sont
# les mêmes dans toutes les langues : S-1-5-18 (SYSTEM) et S-1-5-32-544 (le
# groupe BUILTIN des administrateurs). Ce contrôle interdit le retour des noms.
set -uo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

fichier=e2e/wdio.conf.js
noms="$(grep -nE "icacls|/grant" "$fichier" | grep -E "'(SYSTEM|Administrators|BUILTIN\\\\+Administrators|Administrateurs)(:[A-Z]+)?'" || true)"
if [ -n "$noms" ]; then
  echo "  ✗ $fichier pose des droits icacls par nom de compte, qui dépend de la langue du système :" >&2
  sed 's/^/      /' <<<"$noms" >&2
  echo "      écrire les SID : /grant '*S-1-5-18:F' /grant '*S-1-5-32-544:F'" >&2
  exit 1
fi
if ! grep -qE "S-1-5-18:F" "$fichier" || ! grep -qE "S-1-5-32-544:F" "$fichier"; then
  echo "  ✗ $fichier ne pose plus les droits SYSTEM (S-1-5-18) et administrateurs (S-1-5-32-544) sur administrators_authorized_keys" >&2
  exit 1
fi
echo "  ✓ e2e : droits du sshd Windows par SID, indépendants de la langue"
