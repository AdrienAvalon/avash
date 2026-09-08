#!/usr/bin/env bash
# Contrôle reproductible : la liste des avis RUSTSEC acceptés vit à UN seul
# endroit lu par cargo-audit (.cargo/audit.toml), plus aucune ligne --ignore à
# tenir synchrone, et cette liste colle à celle de deny.toml.
#
# Trouvé par l'audit du 8 septembre 2026 : un fichier `audit.toml` à la racine
# se présentait comme la liste des vulnérabilités acceptées, mais cargo-audit ne
# lit sa config que depuis ./.cargo/audit.toml (puis ~/.cargo/audit.toml). Le
# fichier racine était donc mort ; la liste effective vivait dans les --ignore
# de check.sh et des deux CI, qui divergeaient (RUSTSEC-2024-0429 y figurait,
# pas dans audit.toml). Corrigé en déplaçant le fichier dans .cargo/ et en
# retirant les --ignore ; ce contrôle rougit si l'un de ces défauts revient.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echecs=0

# 1. Le fichier racine mort ne doit pas réapparaître.
if [[ -f audit.toml ]]; then
  echo "  ✗ audit.toml à la racine : fichier mort (cargo-audit lit .cargo/audit.toml)" >&2
  echecs=1
fi

# 2. La config lue par cargo-audit doit exister et lister au moins un avis.
if [[ ! -f .cargo/audit.toml ]]; then
  echo "  ✗ .cargo/audit.toml manquant : cargo-audit n'ignorerait plus rien" >&2
  exit 1
fi

# Les entrées réelles sont des chaînes citées ; les commentaires citent les
# mêmes IDs sans guillemets, on ne prend donc que les guillemetés.
avis_audit="$(grep -oE '"RUSTSEC-[0-9]{4}-[0-9]{4}"' .cargo/audit.toml | tr -d '"' | sort -u)"
avis_deny="$(grep -oE '"RUSTSEC-[0-9]{4}-[0-9]{4}"' deny.toml | tr -d '"' | sort -u)"

if [[ -z "$avis_audit" ]]; then
  echo "  ✗ .cargo/audit.toml : aucun avis dans [advisories] ignore" >&2
  echecs=1
fi

# 3. Les deux sources autoritaires (cargo-audit et cargo-deny) doivent lister
# exactement les mêmes avis, sinon la divergence d'avant revient par une autre
# porte.
if [[ "$avis_audit" != "$avis_deny" ]]; then
  echo "  ✗ .cargo/audit.toml et deny.toml divergent sur les avis acceptés :" >&2
  echo "    audit.toml : $(echo "$avis_audit" | tr '\n' ' ')" >&2
  echo "    deny.toml  : $(echo "$avis_deny" | tr '\n' ' ')" >&2
  echecs=1
fi

# 4. Plus aucune ligne `cargo audit` ne doit porter --ignore : la liste est
# centralisée dans le fichier, un --ignore résiduel la ferait diverger.
for f in check.sh .github/workflows/ci.yml .gitlab-ci.yml; do
  if grep -n 'cargo audit' "$f" | grep -q -- '--ignore'; then
    echo "  ✗ $f : une ligne « cargo audit » porte encore --ignore (à retirer, cf. .cargo/audit.toml)" >&2
    echecs=1
  fi
done

if [[ "$echecs" -ne 0 ]]; then
  exit 1
fi

echo "  ✓ avis cargo-audit centralisés dans .cargo/audit.toml, alignés sur deny.toml, sans --ignore résiduel"
