# Correctifs portés sur IronRDP et vnc-rs

Cinq crates sont copiés ici, chacun avec un ou deux changements ciblés, décrits section par section.

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

## Deux paniques de plus sur le chemin graphique

Le fuzzing par mutation, étendu à un enregistrement GNOME Remote Desktop, a
montré qu'un flux corrompu pouvait arrêter le processus depuis deux endroits :
la décompression ZGFX (`ironrdp-graphics`, indexation d'une fenêtre glissante)
et la conversion YCbCr (la caisse `yuv`, arithmétique sur des coefficients hors
plage). Ces deux paquets ne sont pas portés ici ; le sidecar isole donc les
appels, et une image illisible reste une image illisible au lieu de couper
toutes les sessions ouvertes.

Ce n'est pas une correction en amont mais un confinement, assumé comme tel :
patcher trois bibliothèques tierces pour un décodeur de pixels serait
disproportionné, et le confinement se teste (`progressif.rs`, campagne de
fuzzing).

## GNOME Remote Desktop ferme sans rien dire

Deux changements liés, dans `ironrdp-connector/src/connection.rs` et
`ironrdp-session/src/{x224/mod.rs, redirection.rs}`.

### Le drapeau qui manquait

GNOME Remote Desktop **exige** que le client annonce le pipeline graphique
(`RNS_UD_CS_SUPPORT_DYNVC_GFX_PROTOCOL`). Sans lui, il ferme la connexion avant
d'envoyer `ServerDemandActive` : le client ne voit qu'une désactivation suivie
d'une coupure, sans la moindre explication. C'est exactement le symptôme
qu'avash présentait, et qu'aucune lecture du code n'aurait expliqué — la
réponse est venue d'une [issue du projet Haven][haven-117], trouvée en
cherchant sur le web.

Vérifié dans les deux sens : avec le drapeau, la connexion aboutit ; et son
ajout ne change rien pour Windows Server 2025, un autre Windows et un xrdp, qui
rendent leur image exactement comme avant. Un serveur ne bascule sur EGFX que si
le client ouvre aussi le canal dynamique correspondant.

[haven-117]: https://github.com/GlassHaven/Haven/issues/117

### Le PDU de redirection, illisible

IronRDP connaît le type `ServerRedirect` (0x0A) mais `ShareControlPdu::from_type`
le rejette : « unexpected share control PDU type ». Or c'est ce qu'envoie GNOME
Remote Desktop une fois la session ouverte, pour renvoyer le client vers la
session de l'utilisateur.

Le décodeur vit dans `ironrdp-session/src/redirection.rs`. Un détail vérifié sur
les octets d'un vrai serveur : la charge commence **huit** octets après le début
du PDU — six d'en-tête Share Control, puis deux de remplissage. Avec six, tous
les champs glissaient de deux et le jeton devenait illisible.

Ce qu'un vrai serveur envoie :

    RedirFlags = 0x0001c016
    LoadBalanceInfo (25 o) = "Cookie: msts=2464288595\r\n"

avash lit la demande **et la suit** depuis la 0.5.0 : reconnexion avec le jeton
de routage, puis échange RDSTLS avec les identifiants à usage unique — seul
mécanisme que ces identifiants acceptent. La couche protocole EGFX, absente
d'IronRDP, est écrite dans le sidecar (`egfx.rs`, `progressif.rs`) ; la
bibliothèque n'en fournissait que les **codecs** (`zgfx`, `progressive`,
`clearcodec`).

### Tolérer ce qui s'intercale avant la réactivation

Dans `ironrdp-connector/src/connection_activation.rs`. Après une redirection,
GNOME Remote Desktop glisse d'autres PDU entre le *Deactivate All* et le *Server
Demand Active* attendu — informations de session, disposition des moniteurs,
code d'erreur. IronRDP ne tolérait que le *Deactivate All* et refusait le reste
avec « unexpected Share Control PDU », ce qui fermait la session juste avant
qu'elle ne devienne utilisable. Un client réel les ignore et continue
d'attendre ; c'est ce que fait le paquet porté, en journalisant ce qu'il écarte.

C'est ce correctif qui a rendu lisible le vrai message du serveur : un
`ServerSetErrorInfo(BadCapabilities)`, jusque-là englouti par le refus.

### Les tests du paquet porté ne s'exécutaient pas

`test = false` dans les `Cargo.toml` vendorisés, hérité du dépôt amont. Les
quatorze tests écrits ici — décalage des tuiles, redirection, capacités
précoces, autodétection — passaient pour verts sans jamais tourner, ni en local
ni en intégration continue, y compris derrière les lignes `cargo test -p
ironrdp-session` du fichier de CI, qui ne lançaient rien. Réactivé.

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

## `ironrdp-pdu` — ClearCodec refuse toute image d'un vrai serveur

Dans `src/codecs/clearcodec/bands.rs`. Le décodeur d'une barre verticale courte
(SHORT_VBAR_CACHE_MISS, [MS-RDPEGFX] 2.2.4.1.1.2.1.1.3) lisait ses deux champs
dans l'ordre inverse de la spécification :

```rust
let y_on  = first_word >> 6;      // bits 13:6 — faux
let y_off = first_word & 0x3F;    // bits 5:0  — faux
```

alors que `shortVBarYOn` occupe les huit bits de poids faible et
`shortVBarYOff` les six suivants. La conséquence n'était pas un rendu
approximatif : `yOn` sur huit bits dépasse presque toujours `yOff` sur six, et
le contrôle de cohérence juste en dessous rejetait **chaque** image. Or
ClearCodec est le codec par lequel Windows envoie l'essentiel de son dessin sur
le canal graphique — aucun bureau Windows ne s'y affichait.

Ce qui a rendu le défaut durable : le test amont qui couvre cette fonction
composait son mot d'entrée avec la même formule que le décodeur. Il vérifiait
donc la cohérence du code avec lui-même, jamais avec le protocole, et passait au
vert pendant que le codec refusait tout. Il compose maintenant le mot comme le
fait FreeRDP — vérifié sur son source — et échouerait si l'ordre revenait.

Trouvé en forçant le canal graphique (`AVASH_EGFX=toujours`) contre deux
serveurs Windows réels, puis en comparant l'erreur à l'implémentation de
référence.

### Les tests de ce paquet non plus ne s'exécutaient pas

Même `test = false` hérité de l'amont que pour les deux autres paquets portés,
et **trois cent soixante-treize** tests concernés. Réactivé. Le compte est
vérifié par `verifier-portes.sh`, qui éprouve chaque paquet depuis son propre
répertoire : `cargo test -p` refuse en silence un paquet qui a des dépendances
de développement sans appartenir à l'espace de travail — exactement le cas de
celui-ci.

## `ironrdp-pdu` — RLEX à une seule couleur

Dans `src/codecs/clearcodec/rlex.rs`. Le sous-codec RLEX de ClearCodec
([MS-RDPEGFX] 2.2.4.6.2.2) code chaque segment par un octet compacté
(`stopIndex` sur `floor(log2(paletteCount − 1)) + 1` bits, `suiteDepth` sur le
reste) puis une longueur de série. Le parseur amont traitait la palette à UNE
couleur comme un cas à part, sans octet compacté : il lisait alors les octets
de longueur avec un décalage d'un octet par segment et refusait l'image
(« suite exceeds region pixel count »). FreeRDP (`clear_decompress_subcode_rlex`,
table `CLEAR_LOG2_FLOOR`) donne un bit à `stopIndex` dans ce cas, et garde
l'octet compacté. Windows Server envoie ainsi les coins unis de sa barre des
tâches (14×64, 64×46, 50×64 : neuf refus sur un enregistrement de vingt
secondes). Trouvé en corrélant, dans le journal du canal graphique
(`AVASH_RDP_JOURNAL_EGFX=1`), les refus et les rectangles concernés. Un test
compose un tel segment de 896 pixels.

## `ironrdp-graphics` — le sous-codec NSCodec de ClearCodec manquait

Copie d'`ironrdp-graphics` 0.9.0 avec **un seul ajout**, dans
`src/clearcodec/` : le fichier `nscodec.rs` et le bras `SubcodecId::NsCodec`
de `decode_subcodec_region` dans `mod.rs`, qui était un « pas encore
implémenté » vide. Une région codée ainsi restait donc à zéro dans le
composite, sans erreur : un rectangle noir. Or c'est par NSCodec que Windows
Server envoie les images colorées de petite taille, à commencer par les icônes
de la barre des tâches (68×24, 46×14). `SurfaceToCache` emportait ensuite le
noir et `CacheToSurface` le reposait à chaque redessin de la barre : les
« carrés noirs » signalés par le mainteneur dans un avash Windows affichant un
bureau, reproduits sans réseau par le rejeu de son enregistrement
(`windows-clearcodec-nscodec`, PDU 249 : deux images dont toute la charge est
une région NSCodec).

Le décodeur est un port de `nsc_rle_decode`, `nsc_rle_decompress_data` et
`nsc_decode` de FreeRDP (`libfreerdp/codec/nsc.c`, [MS-RDPNSC]) : quatre
plans (luma, chroma orange, chroma vert, alpha), chacun brut, compressé RLE
ou absent (tout à 0xFF), largeur de travail arrondie à 8 et hauteur à 2 quand
la chroma est sous-échantillonnée, récupération de la perte de couleur par
décalage puis lecture signée, et conversion YCoCg → BGR. Quatre tests
unitaires (RLE, image unie, chroma décalée et sous-échantillonnée, flux
menteurs), plus la fixture.

Les tests du paquet sont réactivés (`test = true`) et comptés par
`verifier-portes.sh`, comme pour les trois autres.

## `vnc-rs` — un serveur VNC hostile, et une file qu'on ne peut pas attendre

Copie de `vnc-rs` 0.5.3 (client RFB : poignée de main, authentification VNC
classique, ZRLE, Tight, Raw, CopyRect), qui sert la session VNC d'avash
(`src/vnc.rs`). Les dépendances de développement (`minifb`, qui tire X11) et
le profil de compilation, ignoré hors racine, sont retirés du `Cargo.toml`.
Trois familles de changements, relevés en relisant le code avant de
l'embarquer, chacun avec son test dans `src/client/tests_hostiles.rs` (un
serveur entier scénarisé dans un tampon) :

- **Ce qu'un serveur fait allouer.** Chaque longueur lue sur le fil (ZRLE,
  Tight, texte du presse-papiers) et chaque rectangle devenaient une
  allocation à la taille dictée par le serveur, dans un `Vec` non initialisé
  (`set_len`) : 65535 × 65535 × 4, soit 17 Gio, et le processus meurt avec la
  session. `codec::tampon` borne toute allocation à 8192 × 8192 × 4 octets et
  la met à zéro ; la résolution annoncée (à l'entrée et par le pseudo-codage
  DesktopSize) est refusée au-delà de 8192 dans chaque dimension ; un
  rectangle qui déborde du cadre est refusé avant qu'un décodeur n'alloue.
- **Deux comportements indéfinis et deux paniques.** Le résultat
  d'authentification (un `u32` du serveur) était transmuté vers une
  énumération à deux variantes ; un message `SetColorMapEntries` tombait sur
  `unimplemented!` ; une liste de types de sécurité vide sur `assert!`. Tout
  cela devient une erreur qui ferme la session en le disant.
- **La file des événements et le verrou.** `recv_event` garde le verrou du
  client pendant toute son attente : une tâche qui attend une image empêche
  toute autre d'envoyer une frappe par `input`, jusqu'à l'image suivante.
  `VncClient::take_events` détache la file ; l'appelant attend sans verrou et
  ne prend le verrou que pour écrire. `set_screen` suit la taille du cadre
  pour que les demandes de mise à jour couvrent tout le bureau après un
  agrandissement.

Et un détail de protocole : le texte du presse-papiers voyage en Latin-1
(RFC 6143, 7.5.6), un octet par caractère ; le paquet envoyait et lisait de
l'UTF-8, et « é » arrivait en « Ã© ».
