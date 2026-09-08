#!/usr/bin/env bash
# Contrôle reproductible : dans scripts/tracer-rdp.sh, le mot de passe du compte
# visé ne doit PAS partir en argument du sidecar (`-p "$MDP"`), lisible dans
# /proc/<pid>/cmdline par tout compte local pendant toute la session, mais lui
# être transmis sur stdin — comme le fait déjà l'application (crates/avash-ui/
# src/rdp.rs) et comme le sait le sidecar (rdp-sidecar/src/args.rs : `-p` absent
# => lecture de la première ligne de stdin).
#
# Trouvé par l'audit du 8 septembre 2026 : tracer-rdp.sh lançait
#   SSLKEYLOGFILE=… timeout "$DUREE" "$RDP" … -u "$USER_" -p "$MDP" …
# Contrairement à conformite.sh qui n'emploie que le compte de test, ce script
# vise un vrai bureau ; son mot de passe restait exposé sur la ligne de commande
# du sidecar (`ps -ef | grep avash-rdp`, collecteurs d'inventaire type osquery)
# durant la capture (20 s par défaut, souvent plus).
#
# Le test extrait du vrai fichier le bloc d'invocation du sidecar et le rejoue
# avec un faux `avash-rdp` qui note son argv et son stdin. Contre le script
# d'origine (`-p "$MDP"`), le mot de passe apparaît dans l'argv : le test rougit.
# Après correctif (alimentation par stdin), l'argv ne le porte plus et stdin oui.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

SCRIPT=scripts/tracer-rdp.sh

# Bloc d'invocation réel : de la ligne qui pose SSLKEYLOGFILE + timeout jusqu'à
# celle qui porte --shot. On reste ancré au script livré (pas de copie figée).
bloc="$(sed -n '/SSLKEYLOGFILE=.*timeout/,/--shot/p' "$SCRIPT")"
[ -n "$bloc" ] || { echo "  ✗ bloc d'invocation du sidecar introuvable dans $SCRIPT" >&2; exit 1; }

banc="$(mktemp -d)"
trap 'rm -rf "$banc"' EXIT

MDP='MDP-SECRET-DU-BUREAU-42'

# Faux sidecar : note son argv (une valeur par ligne) et tout son stdin.
faux_rdp="$banc/avash-rdp"
cat >"$faux_rdp" <<'STUB'
#!/usr/bin/env bash
printf '%s\n' "$@" >"$ARGV_OUT"
cat >"$STDIN_OUT"
STUB
chmod +x "$faux_rdp"

# On rejoue le vrai bloc en neutralisant l'environnement de capture :
#  - timeout : fonction qui jette la durée et exécute la commande ;
#  - "$RDP" -> notre faux sidecar ; les variables du script fixées ici ;
#  - "$@" (options supplémentaires) vide ; stdin du bloc fermé (/dev/null) pour
#    que le faux sidecar ne bloque pas quand rien ne l'alimente (cas d'origine).
ARGV_OUT="$banc/argv" STDIN_OUT="$banc/stdin" \
bash -c '
  set -uo pipefail
  timeout() { shift; "$@"; }
  TRAVAIL="'"$banc"'"
  RDP="'"$faux_rdp"'"
  HOTE=hote; PORT=3389; USER_=compte; MDP="'"$MDP"'"; DUREE=1
  set --
  '"$bloc"'
' </dev/null >/dev/null 2>&1 || true

argv="$(cat "$banc/argv" 2>/dev/null || true)"
entree="$(cat "$banc/stdin" 2>/dev/null || true)"

if printf '%s' "$argv" | grep -qF -- "$MDP"; then
  echo "  ✗ tracer-rdp.sh : le mot de passe part en argument du sidecar (visible dans /proc/<pid>/cmdline)" >&2
  echo "  → retirer « -p \"\$MDP\" » et alimenter stdin : printf '%s\\n' \"\$MDP\" | … \"\$RDP\" …" >&2
  exit 1
fi
if ! printf '%s' "$entree" | grep -qF -- "$MDP"; then
  echo "  ✗ tracer-rdp.sh : le mot de passe n'est pas transmis au sidecar sur stdin" >&2
  echo "  → le sidecar lit la première ligne de stdin quand -p est absent (args.rs)" >&2
  exit 1
fi

echo "  ✓ tracer-rdp.sh : mot de passe transmis au sidecar par stdin, absent de l'argv"
