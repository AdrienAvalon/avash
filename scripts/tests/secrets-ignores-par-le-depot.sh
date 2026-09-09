#!/usr/bin/env bash
# Contrôle reproductible : la protection qui tient `secrets/` hors de git doit
# voyager AVEC le dépôt, pas rester dans un réglage local.
#
# Trouvé par l'audit du 9 septembre 2026. `secrets/` et `.sops.yaml` n'ont
# jamais été suivis, mais la seule règle qui les écartait vivait dans
# `.git/info/exclude`, un fichier propre à ce clone, que ni un autre poste du
# mainteneur, ni un contributeur, ni une machine reconstruite après une panne ne
# reçoivent. Sur un tel clone, un `git add -A` tapé pendant une mise au point
# ajoutait le dossier des jetons sans qu'aucune règle versionnée ne s'y oppose.
#
# On vérifie avec git lui-même (`git check-ignore`), en s'appuyant uniquement
# sur les règles versionnées : un fichier de test est créé dans un dépôt neuf
# qui ne reçoit que le `.gitignore` du projet.
set -uo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT

git -C "$bac" init -q
cp .gitignore "$bac/.gitignore"
mkdir -p "$bac/secrets"
printf 'jeton\n' > "$bac/secrets/github.enc.yaml"
printf 'aide\n' > "$bac/secrets/git-credential-sops.sh"
printf 'creation_rules: []\n' > "$bac/.sops.yaml"

echec=0
for chemin in secrets/github.enc.yaml secrets/git-credential-sops.sh .sops.yaml; do
  if ! git -C "$bac" check-ignore -q "$chemin"; then
    echo "  ✗ « $chemin » n'est pas ignoré par le .gitignore versionné" >&2
    echec=1
  fi
done

# Contrôle en sens inverse : la règle ne doit pas être si large qu'elle avale
# des fichiers légitimes du dépôt.
mkdir -p "$bac/crates/avash/src" "$bac/docs"
printf 'fn main() {}\n' > "$bac/crates/avash/src/lib.rs"
printf '# doc\n' > "$bac/docs/architecture.md"
for chemin in crates/avash/src/lib.rs docs/architecture.md; do
  if git -C "$bac" check-ignore -q "$chemin"; then
    echo "  ✗ « $chemin » est ignoré à tort : la règle est trop large" >&2
    echec=1
  fi
done

if [ "$echec" -ne 0 ]; then
  echo "✗ secrets-ignorés : la protection de secrets/ ne suit pas le dépôt." >&2
  exit 1
fi
echo "✓ secrets-ignorés : secrets/ et .sops.yaml écartés par le .gitignore versionné"
