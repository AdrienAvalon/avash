# Correctif porté sur `ironrdp-session`

Ce répertoire contient une copie de `ironrdp-session` 0.11.0 avec **un seul
changement**, dans `src/fast_path.rs`. Tout le reste est identique à l'amont ;
`diff -r` avec la version de crates.io ne doit signaler que ce fichier.

## Le défaut

RDP autorise `bitmapWidth` à dépasser la largeur du rectangle de destination
(MS-RDPBCGR 2.2.9.1.1.3.1.2.2). xrdp s'en sert : il complète ses tuiles à un
multiple de 4 et annonce, par exemple, 340 pixels de large pour un rectangle
qui n'en fait que 337.

Le décodeur écrit alors le tampon à la largeur du **bitmap**, puis les
fonctions `apply_*` le relisent en le découpant à la largeur du **rectangle**.
Chaque ligne glisse de la différence, et l'image part en biais — d'autant plus
que l'on descend.

Le chemin **non compressé** retire déjà ce remplissage, avec un commentaire qui
cite la spécification. Les chemins **compressés** — RDP 6.0 en 32 bits et RLE
entrelacé — ne le font pas. C'est là le défaut.

## Constaté, pas supposé

Mesuré contre un xrdp réel (SLED-15), en instrumentant le décodeur :

    comprime=true bpp=32 bmp=340x12 rect=337x12   (70 fois)
    comprime=true bpp=32 bmp=352x11 rect=350x11   (38 fois)
    comprime=true bpp=32 bmp=88x30  rect=85x30

Avant le correctif, la fenêtre de connexion xrdp était illisible, cisaillée en
diagonale. Après, elle est nette. Un serveur Windows (Server 2025), qui n'ajoute
jamais ce remplissage, rend exactement pareil avant et après.

## À retirer quand l'amont corrigera

Ce n'est pas un correctif propre à avash : c'est un défaut d'IronRDP qui touche
tout client parlant à xrdp. Dès qu'une version publiée le corrige, supprimer ce
répertoire et la section `[patch.crates-io]` de `rdp-sidecar/Cargo.toml`.
