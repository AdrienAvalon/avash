#!/bin/sh
# Sidecar factice des tests de `src/rdp.rs` (audit du 12 septembre 2026,
# C-couv-2). L'application ne le lance jamais : seuls les tests le passent à
# `ouvrir_avec` à la place d'avash-rdp.
#
# Il note ses arguments, puis joue le scénario que le test a posé dans
# « $TMPDIR/<valeur de --host>/scenario.sh » (un fichier de données, sourcé).
# Écrire un script exécutable par test l'aurait exposé à ETXTBSY : un fil
# voisin qui forke pendant l'écriture garde le descripteur ouvert au moment
# de l'exec. Ce fichier-ci n'est jamais écrit par les tests.
hote=
prec=
for a in "$@"; do
  if [ "$prec" = "--host" ]; then
    hote=$a
  fi
  prec=$a
done
D="${TMPDIR:-/tmp}/$hote"
printf '%s\n' "$@" > "$D/argv"
. "$D/scenario.sh"
