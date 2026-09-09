#!/usr/bin/env bash
# Contrôle reproductible : chaque avis nommé dans la liste `ignore` de deny.toml
# doit correspondre à un paquet réel du graphe.
#
# Trouvé par l'audit du 9 septembre 2026. `RUSTSEC-2024-0429` y était accompagné
# d'un commentaire présentant l'entrée comme un contournement nécessaire : « on
# ne peut ni la retirer ni la corriger », « nommée ici pour que le blocage
# unsound reste actif sur tout le reste ». cargo-deny répondait pourtant
# `advisory-not-detected` : l'avis n'a jamais été rencontré dans ce graphe, donc
# l'entrée ne protégeait rien et le commentaire décrivait une vigilance qui
# n'existait pas. Un garde-fou qui ment est pire qu'un garde-fou absent : on
# cesse d'aller voir.
#
# La règle vaut aussi pour l'avenir : le jour où un avis ignoré cesse de
# s'appliquer (dépendance retirée, correctif remonté), l'entrée doit disparaître
# plutôt que de rester à dormir dans la configuration.
set -uo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

if ! cargo deny --version >/dev/null 2>&1; then
  echo "• deny-ignore : cargo-deny absent de cette machine, contrôle sans objet"
  exit 0
fi

echec=0
for cible in "." "rdp-sidecar"; do
  sortie="$( (cd "$cible" && cargo deny check advisories) 2>&1 || true)"
  # cargo-deny encadre l'entrée fautive : l'identifiant arrive quelques lignes
  # après l'en-tête de l'avertissement, avec le chemin et le numéro de ligne.
  fantomes="$(grep -A6 'advisory-not-detected' <<<"$sortie" | grep -oE 'RUSTSEC-[0-9]{4}-[0-9]{4}' | sort -u || true)"
  if [ -n "$fantomes" ]; then
    echo "  ✗ $cible/deny.toml ignore des avis que cargo-deny ne rencontre pas :" >&2
    sed 's/^/      /' <<<"$fantomes" >&2
    echo "      retirer l'entrée (elle ne protège rien) plutôt que de la laisser prétendre le contraire" >&2
    echec=1
  fi
done

if [ "$echec" -ne 0 ]; then
  echo "✗ deny-ignore : deny.toml décrit une protection qui n'agit pas." >&2
  exit 1
fi
echo "✓ deny-ignore : tout avis ignoré correspond à un paquet réel du graphe"
