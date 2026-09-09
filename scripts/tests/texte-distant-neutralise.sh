#!/usr/bin/env bash
# Contrôle reproductible : le texte venu du réseau ne doit jamais atteindre
# tel quel ni les marqueurs internes, ni le terminal.
#
# Trouvé par l'audit du 9 septembre 2026 (constat critique). Le cœur recopiait
# l'invite `keyboard-interactive` du serveur dans son message d'erreur. Or
# l'interface décide d'un geste lourd, proposer d'OUBLIER la clé d'hôte
# mémorisée, sur la simple présence de `[AVASH_HOST_KEY_CHANGED]` quelque part
# dans ce message. Un serveur hostile n'avait donc qu'à écrire ce marqueur dans
# son invite pour faire effacer la confiance TOFU d'un hôte sain. Le même texte
# arrivait dans xterm.js sans filtre, séquences ANSI comprises.
#
# Deux barrières, une par bout de la chaîne, et ce test les verrouille toutes
# les deux :
#   - côté cœur, `message_prompt_non_supporte` passe par `texte_distant_sur` ;
#   - côté front, tout `term.write` qui interpole une erreur ou une étiquette
#     de cible passe par `nettoyerPourTerminal`, et `markClosed` nettoie son
#     argument.
#
# La relecture du 9 septembre a montré que le durcissement ne couvrait que
# `avash list`, le CLI v0.1 : le Tauri composait la même chaîne
# « utilisateur@hôte:port » depuis ~/.ssh/config et l'écrivait brute dans
# xterm.js, si bien qu'un `HostName srv\x1b]0;PWNED\x07` posé par un autre outil
# rejouait sa séquence à chaque ouverture d'onglet. D'où `etiquetteHote`, seule
# fabrique de cette étiquette, et la règle ci-dessous qui couvre `label` et
# `cible` au même titre que les messages d'erreur.
#
# La règle est vérifiée sur les fichiers réels, puis rejouée sur un contenu
# fautif fabriqué : une règle qui ne rougit sur rien ne prouve rien.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echec=0

# --- Règle front, isolée pour être rejouable sur un contenu de test ----------
# Rend 1 si un `term.write` interpole une erreur sans la neutraliser.
regle_front() { # fichier
  grep -n 'term\.write(' "$1" \
    | grep -E '\$\{[^}]*(String\(|\be\b|\bfe\b|\braison\b|\bmsg\b|\blabel\b|\bcible\b)' \
    | grep -v 'nettoyerPourTerminal' \
    | grep -v 'payload\.data'
}

# --- Cœur : l'invite du serveur passe par le neutralisateur ------------------
if ! grep -A6 'fn message_prompt_non_supporte' crates/avash/src/ssh.rs \
  | grep -q 'texte_distant_sur(prompt)'; then
  echo "  ✗ crates/avash/src/ssh.rs : message_prompt_non_supporte n'appelle plus texte_distant_sur" >&2
  echec=1
fi

# `texte_distant_sur` doit continuer de couvrir les trois vecteurs : marqueurs,
# caractères de contrôle, longueur.
for attendu in 'sans_marqueurs_internes' 'is_control()' 'TEXTE_DISTANT_MAX'; do
  if ! grep -A22 'pub fn texte_distant_sur' crates/avash/src/ssh.rs | grep -q "$attendu"; then
    echo "  ✗ crates/avash/src/ssh.rs : texte_distant_sur ne traite plus « $attendu »" >&2
    echec=1
  fi
done

# --- Front : aucune écriture terminal non neutralisée -----------------------
for source in web/main.ts; do
  if fautives="$(regle_front "$source")" && [ -n "$fautives" ]; then
    echo "  ✗ $source : une erreur atteint le terminal sans nettoyerPourTerminal :" >&2
    echo "$fautives" | sed 's/^/      /' >&2
    echec=1
  fi
done

# L'étiquette « utilisateur@hôte:port » se fabrique en un seul endroit, qui
# neutralise. La recomposer à la main dans main.ts rouvrirait le trou : le
# terminal, la modale de mot de passe et le titre d'onglet la reçoivent tous.
if ! grep -q 'const label = etiquetteHote(h);' web/main.ts; then
  echo "  ✗ web/main.ts : l'étiquette d'hôte ne vient plus d'etiquetteHote" >&2
  echec=1
fi
if ! grep -A3 'export function etiquetteHote' web/filters.ts | grep -q 'nettoyerPourTerminal('; then
  echo "  ✗ web/filters.ts : etiquetteHote ne neutralise plus son étiquette" >&2
  echec=1
fi

# Côté CLI, la même règle : `avash list` imprime des champs relus du fichier.
if ! grep -A16 'fn ligne_hote' crates/avash/src/bin/avash.rs \
  | grep -c 'avash::sans_controle(' | grep -q '^4$'; then
  echo "  ✗ crates/avash/src/bin/avash.rs : ligne_hote ne neutralise plus ses quatre champs" >&2
  echec=1
fi

# `markClosed` neutralise son argument avant de l'écrire et de le poser en
# titre d'onglet.
if ! grep -A8 'function markClosed' web/main.ts | grep -q 'nettoyerPourTerminal(raison)'; then
  echo "  ✗ web/main.ts : markClosed n'appelle plus nettoyerPourTerminal sur son argument" >&2
  echec=1
fi

# Le flux du PTY, lui, doit garder ses séquences : c'est du terminal légitime.
if grep -n 'term\.write(' web/main.ts | grep 'payload\.data' | grep -q 'nettoyerPourTerminal'; then
  echo "  ✗ web/main.ts : le flux du PTY ne doit PAS être filtré, il perdrait ses couleurs" >&2
  echec=1
fi

# --- La règle front sait-elle encore rougir ? -------------------------------
bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT
printf '%s\n' 'term.write(`\r\n⚠️ ${String(e)}\r\n`);' > "$bac/fautif.ts"
if ! regle_front "$bac/fautif.ts" >/dev/null; then
  echo "  ✗ la règle ne détecte plus une écriture fautive : elle ne prouve plus rien" >&2
  echec=1
fi
printf '%s\n' 'term.write(`\r\n⚠️ ${nettoyerPourTerminal(String(e))}\r\n`);' > "$bac/sain.ts"
if regle_front "$bac/sain.ts" >/dev/null; then
  echo "  ✗ la règle rougit sur une écriture pourtant neutralisée" >&2
  echec=1
fi
# Le cas exact de la relecture du 9 septembre : l'étiquette de la cible, que la
# règle ignorait quand elle ne cherchait que des messages d'erreur.
printf '%s\n' 'term.write(`${t("connexion-a", { cible: label })}`);' > "$bac/etiquette.ts"
if ! regle_front "$bac/etiquette.ts" >/dev/null; then
  echo "  ✗ la règle ne détecte plus une étiquette de cible écrite brute" >&2
  echec=1
fi

if [ "$echec" -ne 0 ]; then
  echo "✗ texte-distant-neutralise : le texte du serveur n'est plus neutralisé." >&2
  exit 1
fi
echo "✓ texte-distant-neutralise : marqueurs et séquences ANSI du serveur neutralisés au cœur et au front"
