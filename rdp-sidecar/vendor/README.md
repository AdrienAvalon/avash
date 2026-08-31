# Correctifs portés sur IronRDP

Deux crates sont copiés ici avec un changement chacun.

Une retouche mécanique s'y ajoute, sans rapport avec les défauts : les
`#[expect(lint)]` deviennent `#[allow(lint)]`. Hors de leur espace de travail
d'origine, les lints attendus ne sont pas activés, l'attente devient « non
satisfaite » et bruite chaque compilation. `diff -r` avec les
versions de crates.io ne doit signaler que le fichier nommé dans chaque section.

## `ironrdp-session` — l'affichage part en biais sur xrdp

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


## `ironrdp-session` — un serveur hostile fait tomber le client

Deux défauts distincts dans `src/image.rs` et `src/fast_path.rs`, trouvés par
**fuzzing par mutation sur un enregistrement réel** (voir `src/magnetoscope.rs`).
Un serveur RDP est une entrée non fiable : rien n'oblige celui d'en face à être
bienveillant, ni même correct.

### Écriture hors du tampon

Les fonctions `apply_*` vérifient que le RECTANGLE de destination tient dans
l'image. Rien ne bornait la quantité de données reçue : un serveur envoyant plus
de lignes que le rectangle n'en contient faisait écrire au-delà du tampon.

    index out of bounds: the len is 1228800 but the index is 81776640

Rust arrête l'écriture — il n'y a pas de corruption mémoire — mais le processus
meurt, et avec lui une session établie. Six chemins étaient concernés : aucun ne
bornait le nombre de lignes.

### Débordement sur un rectangle dégénéré

`InclusiveRectangle::width()` calcule `droite - gauche + 1` en supposant
l'invariant `gauche <= droite`. `rect_fits` vérifiait que le rectangle tient
dans l'image, jamais que ses bords sont dans l'ordre. Un rectangle dégénéré
passait la garde puis faisait déborder la soustraction.

Corrigé en deux endroits — la garde elle-même, et le rejet en amont dans le
traitement des mises à jour — parce que la largeur est parfois calculée avant
d'atteindre la garde.

## `ironrdp-connector` — la connexion reste suspendue sans fin

Dans `src/connection.rs`. La détection automatique des caractéristiques réseau
([MS-RDPBCGR] 2.2.14) se déroule ainsi : le serveur envoie *Bandwidth Measure
Start*, une ou plusieurs charges, puis *Stop* — et attend des *Bandwidth Measure
Results*.

La version amont ne répond qu'au RTT. Son commentaire explicite l'hypothèse :

> the server proceeds to licensing whether or not it receives them, so skipping
> them does not stall the sequence

Contre un serveur réel du parc de test, cette hypothèse est fausse. Le serveur
attend les résultats, ne les reçoit jamais, et la séquence reste suspendue là —
pour toujours, sans un mot. La trace le montre sans ambiguïté :

    Wait for PDU  connector.state="ConnectTimeAutoDetection"
    PDU received  length=25
    PDU received  length=16347      <- la charge de mesure
    Wait for PDU  connector.state="ConnectTimeAutoDetection"   <- et plus rien

La mesure est donc réellement effectuée : instant de départ au *Start*, cumul
des octets à chaque charge — **y compris celle que porte le *Stop* lui-même à la
connexion** —, puis réponse avec le délai et le volume.

Après ce correctif, la séquence franchit la licence et atteint l'échange de
capacités. Sur ce serveur-là elle échoue ensuite pour une raison qui lui est
propre, mais elle échoue **en le disant**, au lieu de rester pendue.
