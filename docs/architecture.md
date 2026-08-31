# Architecture technique d'Avash

Avash est un gestionnaire de connexions SSH et RDP construit avec **Tauri 2**
(coquille native Rust) et un front **TypeScript**. Ce document décrit
l'organisation du code et les choix techniques notables.

## Vue d'ensemble

```
+-------------------------------------------------------------+
|  Application Tauri (avash-ui)                               |
|                                                             |
|   Front (web/)                Cœur natif (Rust)             |
|   TypeScript + xterm.js  <->  commands.rs / rdp.rs          |
|      main.ts / filters.ts     (commandes Tauri)            |
|                                     |                       |
|                                     v                       |
|                              crate avash (SSH, SFTP,       |
|                              tunnels, secrets, config)      |
+-------------------------------------|-----------------------+
                                      | lance (stdin: mot de passe)
                                      v
                          +-----------------------+
                          |  Sidecar avash-rdp    |  (processus séparé,
                          |  IronRDP              |   HORS workspace)
                          +-----------------------+
                                      ^
                                      | WebSocket binaire (127.0.0.1)
                                      |
                              Front (canvas RDP)
```

## Le workspace Cargo

Le fichier `Cargo.toml` à la racine définit un workspace de deux crates, et en
**exclut explicitement** le sidecar RDP :

- **`crates/avash`** — le **cœur SSH réutilisable**. Il ne dépend pas de Tauri
  et regroupe toute la logique métier :
  - `lib.rs` — parseur et sérialiseur de `~/.ssh/config` (avec `Include`),
    validation des champs contre l'injection, et `ecrire_atomiquement` — le
    seul chemin d'écriture des fichiers de configuration ;
  - `ssh.rs` — connexion et authentification (russh), vérification TOFU des
    clés d'hôte, redirections de ports ;
  - `sftp.rs` — client SFTP, dont un téléchargement **en bandes parallèles** ;
  - `tunnel.rs` — tunnels SSH ;
  - `snippet.rs` — snippets ;
  - `folders.rs` — arborescence de dossiers (annotations `#Folder:`) ;
  - `secrets.rs` — stockage des mots de passe dans le trousseau (`keyring`) ;
  - `rdphost.rs` — hôtes RDP enregistrés ;
  - `keys.rs`, `osinfo.rs`, `testutil.rs` — utilitaires.

- **`crates/avash-ui`** — l'**application Tauri**. Elle expose le cœur au front
  via des commandes Tauri :
  - `commands.rs` — commandes SSH/SFTP/config/tunnels/snippets ;
  - `rdp.rs` — lancement et supervision du sidecar RDP ;
  - `main.rs` / `lib.rs` — point d'entrée et enregistrement des plugins Tauri
    (`dialog`, `updater`, `process`, `clipboard-manager`).

- **`rdp-sidecar`** (`avash-rdp`) — le **client RDP**, dans un projet Cargo
  **séparé, hors du workspace**. Ce n'est pas un détail de style : IronRDP
  épingle des versions **pré-publication** de `curve25519-dalek` /
  `ed25519-dalek` qui sont **incompatibles** avec celles exigées par `russh`
  (utilisé par le cœur SSH). Cohabiter dans le même arbre de dépendances est
  donc impossible. La séparation en processus distinct résout le conflit :
  chaque binaire résout ses dépendances indépendamment. C'est le sens du
  `exclude = ["rdp-sidecar", "test-rdp-server"]` dans le `Cargo.toml` racine.

Au moment de la release, le sidecar est compilé puis embarqué dans l'AppImage
via le mécanisme `externalBin` de Tauri, à côté de l'exécutable principal.

## Le front

Le front vit dans `web/` (paquet `avash-web`, `type: module`) :

- **`main.ts`** — l'application : rendu de l'arbre des hôtes, onglets, sessions
  de terminal (xterm.js), panneau SFTP, gestion des sessions RDP (canvas et
  WebSocket) ;
- **`filters.ts`** — la logique **pure et testable** extraite du reste : arbre
  des hôtes, correspondance de recherche, scancodes clavier, mappage souris
  RDP (letterbox), lecture des verrous clavier. Couverte par `filters.test.ts` ;
- **`prefs.ts`** — les réglages retenus d'un lancement à l'autre. Isolés du
  module d'entrée précisément pour être exerçables : ils décident de ce qui sort
  de la machine, ce qui les rend trop importants pour n'être couverts que de
  bout en bout ;
- **`index.html`**, `icons.ts` — interface et icônes.

Le terminal repose sur **xterm.js** et ses add-ons (`fit`, `search`,
`web-links`, `webgl`). Le front est bâti par **Vite 8** et vérifié par ESLint
typé et `tsc`. Le binaire release embarque `web/dist` : après toute
modification du front, il faut recompiler `avash-ui`.

Deux comportements de WebKitGTK/WRY ont dicté des choix importants :

- `window.confirm()` ne bloque pas (renvoie une `Promise` toujours vraie) et
  `window.prompt()` renvoie `null`. Avash utilise donc des dialogues maison
  (`askConfirm()` / `askText()`), et `scripts/guard.sh` interdit les dialogues
  natifs ;
- un canvas caché peut perdre son contenu (backing-store) : au retour sur un
  onglet RDP, le front redemande une image complète au sidecar.

## Protocole WebSocket binaire (application <-> sidecar RDP)

Le sidecar `avash-rdp` écoute sur `127.0.0.1:<port éphémère>`, authentifié par
un jeton aléatoire de 64 bits. Il l'annonce sur sa sortie standard sous la forme
`<port> <jeton>` ; l'application lit ces valeurs (voir
`crates/avash-ui/src/rdp.rs`) et ouvre le WebSocket. La **première trame**
envoyée par le front est le jeton.

Le sidecar **boucle** sur les connexions entrantes et rejette celles qui ne
présentent pas le bon jeton, avec un délai de garde par tentative. Il n'en
acceptait qu'une auparavant, et un premier message quelconque le faisait
quitter : le port étant ouvert avant que l'interface n'en soit avertie,
n'importe quel processus local — ou une page web, les WebSocket n'étant pas
soumises à la politique d'origine pour *établir* la connexion — détruisait ainsi
une session RDP déjà authentifiée. Le jeton, lui, n'a jamais été à portée :
c'était un déni de service, pas un détournement.

Ensuite, tout transite en **binaire** (`ArrayBuffer` natif — ni base64, ni
JSON), le premier octet étant le code de message. Les définitions de référence
sont `input_ops` / `frame_msg` dans `rdp-sidecar/src/main.rs` et le
gestionnaire `ws.onmessage` / `send` dans `web/main.ts`.

### Application → sidecar

Entiers en little-endian.

| Code | Message      | Charge utile |
|------|--------------|--------------|
| `1`  | Souris (déplacement) | `x:u16`, `y:u16` |
| `2`  | Souris (bouton) | `bouton:u8`, `pressé:u8`, `x:u16`, `y:u16` |
| `3`  | Molette      | `delta:i16` (+ remplissage) |
| `4`  | Clavier      | `scancode:u16`, `pressé:u8` |
| `5`  | Redimensionner | `largeur:u16`, `hauteur:u16` |
| `6`  | ACK de rendu | (aucune — cadencement adaptatif) |
| `8`  | Presse-papiers (poste → distant) | texte UTF-8 |
| `9`  | Redemander l'image complète | (aucune) |
| `10` | État des verrous clavier | `bits:u8` (Verr. num / maj / défil.) |
| `11` | Onglet visible ou en pause | `pause:u8` |
| `12` | Partage du presse-papiers autorisé | `autorise:u8` |

### Sidecar → application

| Code | Message      | Charge utile |
|------|--------------|--------------|
| `1`  | Connecté / nouvelle taille | `largeur:u16`, `hauteur:u16` |
| `2`  | Rectangle d'écran (FRAME) | `x:u16`, `y:u16`, `w:u16`, `h:u16`, puis pixels RGBA |
| `3`  | Erreur       | message UTF-8 — **réservé**, voir ci-dessous |
| `7`  | Statistiques | `fps:u16`, `débit:u32` (Ko/s), `latence:u16` (ms) |
| `8`  | Presse-papiers (distant → poste) | texte UTF-8 |
| `13` | Écran, plusieurs rectangles | `n:u8`, puis `n` fois (`x:u16`, `y:u16`, `w:u16`, `h:u16`, pixels RGBA) |

Le code `3` est géré par le front mais **n'est jamais émis** : un échec survenu
avant la connexion sort sur l'erreur standard du processus et remonte comme
message d'erreur d'ouverture ; un échec en cours de session ferme le WebSocket,
et l'interface relit les dernières lignes du processus (`rdp_diagnostic`) pour
les afficher dans l'incrustation « Connexion RDP fermée ». Le code reste réservé.


Le code `13` existe parce que le sidecar n'accumulait qu'une **union
englobante** des zones modifiées. Deux poussières aux coins opposés donnaient un
rectangle plein écran. Mesuré sur le fil contre un vrai xrdp, même parcours de
souris, 20 secondes :

| | trames | rectangles | octets |
|---|---|---|---|
| union englobante | 6 | 6 | 8,39 Mo |
| fusion sélective | 9 | 20 | 4,36 Mo |

Moitié moins d'octets, et davantage de trames livrées — donc plus fluide aussi.

La règle de fusion est arithmétique, pas heuristique : deux zones ne fusionnent
que si l'union coûte moins cher que les deux séparées, en-têtes compris. Le
nombre de rectangles est borné à huit ; au-delà, la paire qui gaspille le moins
fusionne, car une trame ne peut pas en porter un nombre illimité.

Un seul rectangle garde la forme historique `2` ; plusieurs empruntent `13`.
Dans les deux cas **une trame, un accusé de rendu** : le cadencement ci-dessous
reste exact.

Le code `6` (ACK de rendu) implémente un **cadencement adaptatif** : le front
accuse réception de chaque frame, et le sidecar n'envoie la suivante qu'une fois
l'ACK reçu (avec un `ACK_TIMEOUT` de garde), ce qui évite d'inonder un poste
plus lent que le débit du serveur.

Le code `11` complète ce cadencement pour les onglets d'arrière-plan. Un onglet
masqué accusait quand même réception de chaque trame : le sidecar y voyait la
voie libre et poussait sans relâche des images entières — 8 Mo en 1080p — vers
un canvas invisible. En pause, il accumule le rectangle sale sans rien émettre ;
le `9` envoyé au retour au premier plan lève la pause, de sorte qu'un oubli côté
interface ne peut pas geler le flux.

Le code `12` transmet au sidecar le réglage de partage du presse-papiers. Sans
lui, le sidecar réclamait au serveur le contenu de son presse-papiers à chaque
annonce de copie, même quand l'interface n'avait plus le droit de l'appliquer.

## Choix techniques notables

- **SSH via russh**, en conservant volontairement la prise en charge RSA
  (dépendance `rsa`, concernée par RUSTSEC-2023-0071 / attaque Marvin, sans
  correctif amont). La compatibilité avec les serveurs et clés RSA a été jugée
  plus importante que ce canal temporel, exploitable seulement par un serveur
  malveillant. Le compromis et l'alternative (compilation sans RSA) sont
  documentés dans `crates/avash/Cargo.toml`, et l'avis est explicitement ignoré
  dans `cargo audit`.
- **russh sans `aws-lc-rs`, avec `ring`** : même fonctionnement vérifié contre
  un vrai `sshd`, mais un binaire final plus léger (~2 Mo de moins).
- **RDP via IronRDP** dans un sidecar, avec le canal **Display Control (DVC)**
  pour le redimensionnement natif et **CLIPRDR** pour le presse-papiers.
- **Vérification des clés d'hôte TOFU** stricte, des deux côtés. Côté SSH, la
  décision est prise par `juger_cle_hote` sur les clés enregistrées, et **non**
  par le booléen de `check_known_hosts` : celui-ci confond « algorithme
  différent » et « hôte inconnu », si bien qu'une clé changée passait pour un
  premier contact. Les marqueurs `@revoked` et `@cert-authority` font refuser la
  connexion — russh les ignore, et une clé marquée compromise aurait été
  réapprise. Côté RDP, l'empreinte SHA-256 de la **clé publique** du serveur est
  épinglée dans `~/.config/avash/rdp_known_hosts`, et la vérification a lieu
  **avant** CredSSP : après, les identifiants seraient déjà partis. On épingle
  la clé et non le certificat entier, pour qu'une reconduction ne déclenche pas
  de fausse alerte.
- **Pas de repli NLA → TLS.** IronRDP n'annonce `PROTOCOL_SSL` que si on le lui
  demande, et le faire revient à dire au serveur qu'on accepte de sauter NLA —
  auquel cas le mot de passe part dans le Client Info PDU, sans authentification
  mutuelle. Avash n'annonce que `HYBRID` : un serveur incapable de NLA fait
  échouer la négociation, ce qui est le bon comportement.
- **`TCP_NODELAY` sur les sessions SSH.** russh laisse l'algorithme de Nagle
  actif par défaut ; sur une session interactive, chaque frappe attendait
  l'accusé du segment précédent. OpenSSH pose ce drapeau sans condition.
- **Écritures atomiques** (`ecrire_atomiquement`) pour tout fichier de
  configuration : temporaire créé en 0600 dans le même répertoire, synchronisé,
  puis renommé. La fonction suit les liens symboliques — un renommage
  remplacerait le lien, et une configuration de dotfiles deviendrait
  silencieusement orpheline — et refuse d'écraser une cible en lecture seule.
- **`WEBKIT_DISABLE_COMPOSITING_MODE` sous Linux.** Le redimensionnement de la
  fenêtre était saccadé : WebKitGTK réallouait ses tampons GPU à chaque image du
  geste (42 % du temps dans le noyau, mesuré au profileur). Sans compositing
  accéléré, la part noyau retombe à 19 % et le débit RDP est inchangé.
- **Secrets dans le trousseau du système** (`keyring`), jamais en clair sur le
  disque ; mot de passe RDP passé au sidecar par stdin.
- **Build release optimisé** : `opt-level = 3`, LTO, `codegen-units = 1`,
  `strip` (workspace et sidecar). Le chemin chaud du sidecar est le décodage
  graphique, d'où le LTO complet.
- **Distribution** : un artefact autonome par système (AppImage sous Linux,
  installeur NSIS sous Windows). Voir `RELEASE.md`.

## Références de fichiers

| Sujet | Fichier |
|-------|---------|
| Parseur / écriture `~/.ssh/config`, validation | `crates/avash/src/lib.rs` |
| Connexion SSH, TOFU, redirections | `crates/avash/src/ssh.rs` |
| Secrets (trousseau) | `crates/avash/src/secrets.rs` |
| Dossiers | `crates/avash/src/folders.rs` |
| Commandes Tauri | `crates/avash-ui/src/commands.rs` |
| Lancement du sidecar RDP | `crates/avash-ui/src/rdp.rs` |
| Sidecar RDP (protocole, IronRDP) | `rdp-sidecar/src/main.rs` |
| Front (application) | `web/main.ts` |
| Front (logique pure testable) | `web/filters.ts` |
| Front (réglages persistants) | `web/prefs.ts` |
| Client SFTP (dont bandes parallèles) | `crates/avash/src/sftp.rs` |
| Validation qualité | `check.sh`, `scripts/guard.sh` |
| Tests de bout en bout | `e2e/` |

## Correctifs portés sur IronRDP

`rdp-sidecar/vendor/` contient deux crates d'IronRDP copiés, avec **un seul
changement chacun**. Ce n'est pas un fork de confort : ce sont deux défauts qui
touchent tout client IronRDP parlant à xrdp, et qui rendaient avash inutilisable
contre des serveurs légitimes.

| Crate | Défaut | Symptôme |
|---|---|---|
| `ironrdp-session` | le remplissage de fin de ligne n'est pas retiré dans les chemins compressés | image cisaillée en diagonale |
| `ironrdp-connector` | la mesure de bande passante n'est jamais renvoyée | connexion suspendue sans fin |

Chacun a ses propres tests, exécutés par `check.sh`, le hook de pré-commit et
les deux chaînes d'intégration : sans cela, une montée de version pourrait les
défaire en silence. `rdp-sidecar/vendor/README.md` dit quoi, pourquoi, et quand
les retirer.

## Le parc RDP local

Trois défauts RDP ont été signalés par l'usage et par lui seul. Les tests
unitaires vérifiaient nos fonctions ; la suite bout en bout vérifiait
l'interface. Entre les deux : le dialogue réel avec un serveur RDP, que rien
n'éprouvait.

`tests-parc/` monte de vrais serveurs xrdp en conteneur, avec **deux bureaux** —
XFCE et GNOME. Ils ne dessinent pas de la même façon, et c'est cette diversité
qui fait sortir les défauts de décodage : le cisaillement n'apparaît que lorsque
le serveur complète ses tuiles à un multiple de quatre.

Le détecteur d'image (`tests-parc/detecteur-cisaillement.py`) mérite un mot. Une
image cisaillée reste *plausible* : bonnes couleurs, bonne disposition générale.
Ni une somme de contrôle, ni une moyenne de pixels, ni une comparaison de taille
ne l'auraient vue. Le détecteur cherche autre chose : dans une image
d'interface, deux lignes voisines se superposent au mieux avec un décalage nul ;
sous cisaillement, l'alignement optimal devient une constante non nulle. Mesuré
sur de vraies captures, la séparation est franche — `+0` sur 100 % des lignes
quand c'est sain, `-2` sur 96 % quand ça ne l'est pas.
