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

# 6. Le processus RDP a sa propre configuration, lue quand on l'audite depuis
# son dossier. Trouvé par l'audit du 12 septembre 2026 (C-chaine-12) : sans
# rdp-sidecar/.cargo/audit.toml, `cargo audit` lancé dans rdp-sidecar/
# signalait RUSTSEC-2023-0071, alors que la chaîne, lancée depuis la racine
# avec --file, lisait la liste de l'espace de travail : le verdict dépendait du
# répertoire. Les chaînes auditent maintenant le sidecar depuis son dossier,
# et sa liste obéit aux mêmes règles que celle de la racine face à son
# deny.toml.
if [[ ! -f rdp-sidecar/.cargo/audit.toml ]]; then
  echo "  ✗ rdp-sidecar/.cargo/audit.toml manquant : l'audit du sidecar dépend du dossier d'où on le lance" >&2
  echecs=1
else
  avis_audit_rdp="$(grep -oE '"RUSTSEC-[0-9]{4}-[0-9]{4}"' rdp-sidecar/.cargo/audit.toml | tr -d '"' | sort -u)"
  avis_deny_rdp="$(grep -oE '"RUSTSEC-[0-9]{4}-[0-9]{4}"' rdp-sidecar/deny.toml | tr -d '"' | sort -u)"
  mentions_deny_rdp="$(grep -oE 'RUSTSEC-[0-9]{4}-[0-9]{4}' rdp-sidecar/deny.toml | sort -u)"
  while read -r avis; do
    [[ -n "$avis" ]] || continue
    if ! grep -qx "$avis" <<<"$avis_audit_rdp"; then
      echo "  ✗ rdp-sidecar/deny.toml ignore $avis, absent de rdp-sidecar/.cargo/audit.toml" >&2
      echecs=1
    fi
  done <<<"$avis_deny_rdp"
  while read -r avis; do
    [[ -n "$avis" ]] || continue
    if ! grep -qx "$avis" <<<"$avis_deny_rdp" && ! grep -qx "$avis" <<<"$mentions_deny_rdp"; then
      echo "  ✗ rdp-sidecar/.cargo/audit.toml ignore $avis, que rdp-sidecar/deny.toml n'ignore pas sans dire pourquoi" >&2
      echecs=1
    fi
  done <<<"$avis_audit_rdp"
fi

# 7. Les trois chaînes auditent le sidecar depuis son dossier : lancé depuis la
# racine avec --file, c'est la liste de l'espace de travail qui le gouverne.
for f in check.sh .github/workflows/ci.yml .gitlab-ci.yml; do
  if grep -n 'cargo audit' "$f" | grep -q -- '--file rdp-sidecar/Cargo.lock'; then
    echo "  ✗ $f : le sidecar est audité depuis la racine (--file), sous la liste de l'espace de travail" >&2
    echecs=1
  fi
  if ! grep -qE '(cd rdp-sidecar.*cargo audit|"\$SIDECAR" cargo audit)' "$f"; then
    echo "  ✗ $f : aucun audit du sidecar lancé depuis rdp-sidecar/" >&2
    echecs=1
  fi
done

# 8. Même verdict partout, constaté : si cargo-audit et sa base d'avis sont
# présents, l'audit lancé depuis rdp-sidecar/ doit passer, hors ligne.
base="${CARGO_HOME:-$HOME/.cargo}/advisory-db"
if cargo audit --version >/dev/null 2>&1 && [[ -d "$base" ]]; then
  if ! (cd rdp-sidecar && cargo audit --no-fetch --deny unsound >/dev/null 2>&1); then
    echo "  ✗ cargo audit lancé depuis rdp-sidecar/ échoue (config du sidecar absente ou avis nouveau)" >&2
    echecs=1
  fi
else
  echo "  • cargo-audit ou sa base absents : verdict du sidecar depuis son dossier non rejoué"
fi

if [[ "$echecs" -ne 0 ]]; then
  exit 1
fi

echo "  ✓ avis cargo-audit centralisés (racine et sidecar), deny.toml sans avis inconnu ni écart muet, sans --ignore résiduel, sidecar audité depuis son dossier"
