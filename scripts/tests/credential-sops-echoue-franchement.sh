#!/usr/bin/env bash
# Contrôle reproductible : l'aide d'identifiants git ne doit JAMAIS répondre des
# identifiants vides comme s'ils étaient valides.
#
# Trouvé par l'audit du 9 septembre 2026. `secrets/git-credential-sops.sh`
# faisait `printf 'username=%s\n' "$(sops -d …)"` sans regarder le code de
# retour de `sops`. Quand le déchiffrement échoue (clé age absente sur une
# machine neuve, clé rotée sans réencoder le fichier, exactement l'opération
# déjà faite pour le jeton GitLab divulgué), la substitution ne capture que la
# sortie standard, donc rien, et git reçoit `username=` / `password=` vides. Il
# les présente à l'hébergeur, se fait refuser, et l'utilisateur lit « échec
# d'authentification » au lieu de « ta clé de déchiffrement manque ».
#
# Le vrai `sops` n'est jamais appelé ici : un faux, placé en tête de PATH, joue
# l'échec puis le succès. Aucun secret réel n'entre dans ce test.
#
# L'aide vit dans `secrets/`, qui n'est pas suivi par git (voir .gitignore) :
# sur un clone qui ne l'a pas, le test se déclare sans objet plutôt que de
# rougir.
set -uo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

AIDE="secrets/git-credential-sops.sh"
if [ ! -f "$AIDE" ]; then
  echo "• credential-sops : $AIDE absent de ce clone, contrôle sans objet"
  exit 0
fi

bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT
mkdir -p "$bac/secrets" "$bac/bin"
cp "$AIDE" "$bac/secrets/"
# Un fichier chiffré factice : l'aide vérifie seulement qu'il existe.
printf 'github_user: ENC[factice]\n' > "$bac/secrets/github.enc.yaml"

# Faux sops : `MODE` décide de son comportement.
cat > "$bac/bin/sops" <<'FAUX'
#!/bin/sh
case "$MODE" in
  echec)  echo "Could not decrypt with AES_GCM" >&2; exit 1 ;;
  vide)   exit 0 ;;
  succes) case "$*" in *_user*) echo "utilisateur-factice" ;; *) echo "jeton-factice" ;; esac ;;
esac
FAUX
chmod +x "$bac/bin/sops"

echec=0

joue() { # MODE -> écrit stdout dans $bac/out, stderr dans $bac/err, rend le code
  MODE="$1" PATH="$bac/bin:$PATH" \
    sh "$bac/secrets/git-credential-sops.sh" get \
    >"$bac/out" 2>"$bac/err" <<<"protocol=https
host=github.com
"
}

# --- 1. sops échoue : rien sur la sortie standard, une explication sur l'erreur
joue echec
code=$?
if [ -s "$bac/out" ]; then
  echo "  ✗ sops en échec : l'aide a quand même répondu des identifiants :" >&2
  sed 's/^/      /' "$bac/out" >&2
  echec=1
fi
if [ "$code" -eq 0 ]; then
  echo "  ✗ sops en échec : l'aide sort en 0, git croit que tout va bien" >&2
  echec=1
fi
# On exige le mot de l'aide elle-même, pas seulement celui de sops : c'est elle
# qui sait quel fichier et quelle clé sont en cause.
if ! grep -q 'git-credential-sops' "$bac/err"; then
  echo "  ✗ sops en échec : l'aide n'explique rien de son côté sur la sortie d'erreur" >&2
  sed 's/^/      /' "$bac/err" >&2
  echec=1
fi

# --- 2. sops réussit mais ne rend rien (clé absente du fichier) --------------
joue vide
if grep -qE '^(username|password)=$' "$bac/out"; then
  echo "  ✗ valeur vide : l'aide la présente quand même à git" >&2
  echec=1
fi

# --- 3. cas nominal : l'aide répond bien ------------------------------------
joue succes
if ! grep -q '^username=utilisateur-factice$' "$bac/out" \
  || ! grep -q '^password=jeton-factice$' "$bac/out"; then
  echo "  ✗ cas nominal cassé : l'aide ne rend plus les identifiants" >&2
  sed 's/^/      /' "$bac/out" >&2
  echec=1
fi

# --- 4. hôte inconnu : on se tait, sans erreur ------------------------------
MODE=succes PATH="$bac/bin:$PATH" \
  sh "$bac/secrets/git-credential-sops.sh" get >"$bac/out" 2>"$bac/err" <<<"host=exemple.invalide
"
if [ -s "$bac/out" ]; then
  echo "  ✗ hôte inconnu : l'aide répond quelque chose" >&2
  echec=1
fi

if [ "$echec" -ne 0 ]; then
  echo "✗ credential-sops : l'aide peut encore rendre des identifiants vides." >&2
  exit 1
fi
echo "✓ credential-sops : échec de déchiffrement signalé franchement, cas nominal intact"
