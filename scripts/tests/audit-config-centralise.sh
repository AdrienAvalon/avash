#!/usr/bin/env bash
# Contrôle reproductible : la liste des avis RUSTSEC acceptés vit à UN seul
# endroit lu par cargo-audit (.cargo/audit.toml), plus aucune ligne --ignore à
# tenir synchrone, et deny.toml n'en accepte aucun qui n'y soit.
#
# Trouvé par l'audit du 8 septembre 2026 : un fichier `audit.toml` à la racine
# se présentait comme la liste des vulnérabilités acceptées, mais cargo-audit ne
# lit sa config que depuis ./.cargo/audit.toml (puis ~/.cargo/audit.toml). Le
# fichier racine était donc mort ; la liste effective vivait dans les --ignore
# de check.sh et des deux CI, qui divergeaient (RUSTSEC-2024-0429 y figurait,
# pas dans audit.toml). Corrigé en déplaçant le fichier dans .cargo/ et en
# retirant les --ignore ; ce contrôle rougit si l'un de ces défauts revient.
#
# Précisé par l'audit du 9 septembre 2026 : les deux outils ne voient pas les
# mêmes avis. RUSTSEC-2024-0429 (glib) est porté par une fonction précise ;
# cargo-audit le rencontre, cargo-deny jamais, et une entrée `ignore` que
# cargo-deny ne rencontre pas est un fantôme que refuse
# scripts/tests/deny-ignore-sans-avis-fantome.sh. Exiger l'égalité stricte des
# deux listes forçait donc ce fantôme. La règle devient : tout avis ignoré par
# deny.toml doit l'être aussi par cargo-audit (sinon la divergence d'avant
# revient par une autre porte), et tout avis que cargo-audit ignore sans que
# deny.toml le fasse doit être NOMMÉ dans un commentaire de deny.toml qui dit
# pourquoi : la différence est alors une décision écrite, pas un oubli.
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

# 3. Tout avis ignoré par cargo-deny doit l'être par cargo-audit : deny.toml ne
# peut pas accepter en douce ce que la liste centrale ne connaît pas.
while read -r avis; do
  [[ -n "$avis" ]] || continue
  if ! grep -qx "$avis" <<<"$avis_audit"; then
    echo "  ✗ deny.toml ignore $avis, absent de .cargo/audit.toml (la liste centrale)" >&2
    echecs=1
  fi
done <<<"$avis_deny"

# 4. Un avis ignoré par cargo-audit mais pas par cargo-deny doit être une
# décision écrite dans deny.toml : son identifiant y figure hors guillemets,
# dans le commentaire qui explique pourquoi cargo-deny ne le rencontre pas.
mentions_deny="$(grep -oE 'RUSTSEC-[0-9]{4}-[0-9]{4}' deny.toml | sort -u)"
while read -r avis; do
  [[ -n "$avis" ]] || continue
  if ! grep -qx "$avis" <<<"$avis_deny" && ! grep -qx "$avis" <<<"$mentions_deny"; then
    echo "  ✗ .cargo/audit.toml ignore $avis, que deny.toml n'ignore pas sans dire pourquoi (le nommer dans un commentaire de [advisories])" >&2
    echecs=1
  fi
done <<<"$avis_audit"

# 5. Plus aucune ligne `cargo audit` ne doit porter --ignore : la liste est
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

echo "  ✓ avis cargo-audit centralisés dans .cargo/audit.toml, deny.toml sans avis inconnu ni écart muet, sans --ignore résiduel"
