#!/usr/bin/env bash
# Contrôle reproductible : la liste d'autorisation de gitleaks doit couvrir la
# valeur factice de test, et RIEN d'autre.
#
# Trouvé par l'audit du 9 septembre 2026. La règle ajoutée le 8 septembre pour
# faire taire gitleaks sur le jeton factice d'un test du sidecar
# (`0123456789abcdef`) était écrite sans ancres. Or gitleaks évalue ces regexes
# en correspondance libre : tout secret CONTENANT cette suite était donc blanchi,
# où qu'il soit dans le dépôt. Le CHANGELOG affirmait « sans rien relâcher
# d'autre » ; c'était faux, et mesuré ici : une clé d'API de trente-six
# caractères commençant par ce motif passait la chaîne Sécurité sans un mot.
#
# Les deux comportements sont vérifiés avec le VRAI gitleaks et la VRAIE
# configuration du dépôt :
#   1. une valeur exactement égale au jeton factice reste autorisée ;
#   2. une clé plus longue qui ne fait que contenir ce motif est signalée.
# Sans gitleaks sur la machine, le contrôle se déclare sans objet (la chaîne
# Sécurité, elle, l'a toujours).
set -uo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if ! command -v gitleaks >/dev/null 2>&1; then
  echo "• gitleaks-allowlist : gitleaks absent de cette machine, contrôle sans objet"
  exit 0
fi

CONFIG="$PWD/.gitleaks.toml"
bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT
echec=0

# Le motif factice est lu dans la configuration elle-même : si quelqu'un change
# la valeur autorisée, le test suit au lieu de mentir.
motif="$(sed -n "s/.*'''\\^\\?\\([0-9a-f]\\{8,\\}\\)\\\$\\?'''.*/\\1/p" "$CONFIG" | head -1)"
if [ -z "$motif" ]; then
  echo "  ✗ aucune valeur factice lisible dans .gitleaks.toml" >&2
  exit 1
fi

scanne() { # dossier -> nombre de constats
  local d="$1"
  gitleaks detect --no-git --source "$d" --config "$CONFIG" \
    --report-format json --report-path "$d/rapport.json" >/dev/null 2>&1 || true
  python3 - "$d/rapport.json" <<'PY'
import json, sys, os
p = sys.argv[1]
print(len(json.load(open(p))) if os.path.exists(p) else 0)
PY
}

# --- 1. La valeur exacte reste autorisée ------------------------------------
# On reproduit la forme qui a fait rougir la 0.10.0 : le jeton factice seul,
# reconnu par la règle générique de gitleaks comme une clé d'API.
mkdir -p "$bac/exact"
printf 'const apiKey = "%s";\n' "$motif" > "$bac/exact/cas.js"
if [ "$(scanne "$bac/exact")" -ne 0 ]; then
  echo "  ✗ la valeur factice « $motif » n'est plus autorisée : la chaîne Sécurité va rougir" >&2
  echec=1
fi

# --- 2. Une vraie clé qui contient le motif doit être signalée ---------------
mkdir -p "$bac/vraie"
printf 'const apiKey = "%sX7q2ZmPl9vB3nK8sT1wR";\n' "$motif" > "$bac/vraie/cas.js"
if [ "$(scanne "$bac/vraie")" -eq 0 ]; then
  echo "  ✗ une clé de 36 caractères contenant « $motif » est blanchie : l'autorisation est trop large" >&2
  echo "      elle doit être ancrée sur la valeur exacte (^…\$), pas laissée en correspondance libre" >&2
  echec=1
fi

if [ "$echec" -ne 0 ]; then
  echo "✗ gitleaks-allowlist : l'autorisation ne couvre pas exactement la valeur factice." >&2
  exit 1
fi
echo "✓ gitleaks-allowlist : le jeton factice est autorisé, un vrai secret qui le contient reste signalé"
