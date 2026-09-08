#!/usr/bin/env bash
# Contrôle reproductible : les justifications de RUSTSEC-2023-0071 (attaque
# Marvin sur `rsa`) dans les deux deny.toml disent la vérité sur l'arbre de
# dépendances qu'elles gouvernent.
#
# Trouvé par l'audit du 8 septembre 2026 : deny.toml (racine) prétendait que
# `rsa` arrivait « par une dépendance transitive de russh que nous n'employons
# pas pour du RSA privé ». Or crates/avash/Cargo.toml active explicitement la
# fonctionnalité `rsa` de russh pour garder les clés id_rsa et les clés d'hôte
# RSA : signer le défi d'authentification avec la clé RSA de l'utilisateur EST
# une opération à clé privée, précisément celle que vise Marvin. La
# justification niait donc le vrai risque (arbitrage documenté dans Cargo.toml).
# Pire, rdp-sidecar/deny.toml recopiait mot pour mot ce commentaire nommant
# russh, alors que russh n'est PAS une dépendance du sidecar : `rsa` y arrive
# par picky (← sspi/ironrdp-connector), pour de la vérification de signatures
# X.509, sans clé privée RSA côté client. Ce contrôle rougit contre ces deux
# formulations et garde l'exactitude de chacune.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echecs=0

# Extrait le bloc de commentaires `#` qui précède immédiatement l'entrée
# "RUSTSEC-2023-0071" dans un deny.toml (sa justification propre).
justification() {
  awk '
    /^[[:space:]]*#/ { buf = buf $0 "\n"; next }
    /"RUSTSEC-2023-0071"/ { printf "%s", buf; exit }
    { buf = "" }
  ' "$1"
}

just_racine="$(justification deny.toml)"
just_sidecar="$(justification rdp-sidecar/deny.toml)"

if [[ -z "$just_racine" || -z "$just_sidecar" ]]; then
  echo "  ✗ justification RUSTSEC-2023-0071 introuvable dans l'un des deny.toml" >&2
  exit 1
fi

# 1. deny.toml racine : le crate `avash` emploie bel et bien `rsa` pour du RSA
# PRIVÉ (signature du défi SSH avec la clé de l'utilisateur). La justification
# ne doit plus prétendre le contraire, et doit renvoyer à l'arbitrage réel.
if echo "$just_racine" | grep -qiE "n'employons pas|pas pour du rsa priv"; then
  echo "  ✗ deny.toml : la justification nie encore l'usage privé de rsa (faux, cf. Cargo.toml features rsa)" >&2
  echecs=1
fi
if ! echo "$just_racine" | grep -qiE 'id_rsa|Cargo\.toml'; then
  echo "  ✗ deny.toml : la justification devrait renvoyer à l'arbitrage id_rsa/Cargo.toml" >&2
  echecs=1
fi

# 2. rdp-sidecar/deny.toml : russh n'est pas dans l'arbre du sidecar (vérifié
# dynamiquement) ; le nommer dans la justification est faux. `rsa` y vient de
# picky, la justification doit le dire.
russh_dans_sidecar="$(grep -c '^name = "russh"' rdp-sidecar/Cargo.lock || true)"
if [[ "$russh_dans_sidecar" -eq 0 ]] && echo "$just_sidecar" | grep -qi 'russh'; then
  echo "  ✗ rdp-sidecar/deny.toml : la justification nomme russh, absent de l'arbre du sidecar" >&2
  echecs=1
fi
if ! echo "$just_sidecar" | grep -qi 'picky'; then
  echo "  ✗ rdp-sidecar/deny.toml : la justification devrait décrire la chaîne picky (X.509)" >&2
  echecs=1
fi

if [[ "$echecs" -ne 0 ]]; then
  exit 1
fi

echo "  ✓ justifications RUSTSEC-2023-0071 exactes (racine : id_rsa/RSA privé ; sidecar : picky/X.509)"
