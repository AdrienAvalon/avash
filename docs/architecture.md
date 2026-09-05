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
|   TypeScript + xterm.js  <->  commands/ · rdp.rs            |
|      main.ts + modules        (commandes Tauri)            |
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
  - `sftp.rs` — client SFTP : téléchargement **en bandes parallèles**, envoi
    séquentiel pipeliné (les bandes montantes ont été mesurées et refusées),
    dossiers entiers, reprise par carte (`.part.reprise` : bandes complètes ;
    `.envoi.reprise` : dernier point de contrôle vidé), annulation
    coopérative, relais d'un serveur à un autre sans écrire sur le poste ;
  - `tunnel.rs` — tunnels SSH ;
  - `snippet.rs` — snippets ;
  - `folders.rs` — arborescence de dossiers (annotations `#Folder:`) ;
  - `secrets.rs` — stockage des mots de passe dans le trousseau (`keyring`),
    et sa sonde pour le diagnostic ;
  - `rdphost.rs` — hôtes RDP enregistrés ;
  - `serie.rs` — port série (`serialport`, sans libudev) : deux fils
    bloquants, un par sens, qui parlent au reste par les canaux d'une session
    SSH ; le chemin doit mener sous `/dev` (ou être un `COMn`) ;
  - `keys.rs`, `osinfo.rs`, `testutil.rs` — utilitaires.

- **`crates/avash-ui`** — l'**application Tauri**. Elle expose le cœur au front
  via des commandes Tauri :
  - `commands/` — les commandes Tauri, un fichier par domaine : `sessions`
    (magasin, cible, connexion, relais, PTY), `sftp`, `tunnels`, `snippets`,
    `enregistrement`, `cles`, `secrets`, `dossiers`, `import`, `sante`,
    `serie` (une session série est une session du magasin comme les autres,
    sans SFTP ni commande à distance), `onglets` (la mémoire de ce qui était
    ouvert, `onglets.json`), `diagnostic` (le texte exporté pour un ticket) ;
    `mod.rs` réexporte tout, si bien que `commands::x` reste le chemin de chaque
    commande ;
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
  `exclude = ["rdp-sidecar", "test-rdp-server", "test-vnc-server", "fuzz"]`
  dans le `Cargo.toml` racine.

  Le même processus sert aussi les bureaux **VNC** (`--vnc`, `src/vnc.rs`) :
  client `vnc-rs` (copie portée dans `vendor/`, voir son README), même poste
  local, même protocole avec l'interface, même image et même cadencement sur
  accusé de réception que le RDP. Seuls changent le dialogue avec le serveur
  et le clavier, en keysyms (message `14`). Quand le serveur offre VeNCrypt,
  le client porté rend le flux à `src/vnc_tls.rs`, qui monte TLS (le même
  rustls que le RDP) et épingle le certificat sous `vnc:<hôte>:<port>` dans
  le fichier des empreintes, avant l'authentification.

Au moment de la release, le sidecar est compilé puis embarqué dans l'AppImage
via le mécanisme `externalBin` de Tauri, à côté de l'exécutable principal.

## Le front

Le front vit dans `web/` (paquet `avash-web`, `type: module`) :

- **`main.ts`** — le cœur de l'application : arbre des hôtes, onglets,
  sessions de terminal (xterm.js), palette, et l'amorçage en toute fin, une
  fois les autres modules importés ;
- **un module par domaine**, chacun important ce qu'il utilise des autres :
  `etat.ts` (l'état partagé — hôtes, sessions, bureaux RDP, réglages — les
  thèmes du terminal et l'accès au DOM), `theme.ts`, `sftp.ts` (panneau,
  transferts, glisser-déposer), `rdp.ts` (sessions sur canvas et WebSocket,
  entrées, presse-papiers, bureaux enregistrés), `tunnels.ts`, `snippets.ts`,
  `dialogues.ts` (saisie, confirmation, mot de passe, piège de focus),
  `notifications.ts`, `connexion-directe.ts`, `cles.ts`, `menu-hote.ts`,
  `dossiers.ts`, `raccourcis.ts`, `terminal-outils.ts` (zoom, recherche, menu
  clic droit), `verrous.ts`, `titre.ts`, `maj.ts`, `panneaux.ts`, `import.ts`
  (import PuTTY et MobaXterm), `enregistrements.ts` (liste des enregistrements
  asciicast), `i18n.ts` (dictionnaires français et anglais, `t()`, application
  à la page), `collage.ts` (décision de collage sûr, testée sans terminal),
  `vue-partagee.ts` (le seul endroit qui décide ce que la zone centrale
  montre : l'onglet actif, ou deux volets), `onglets-memoire.ts` (pur : ce
  qui vaut la peine d'être retenu) et `onglets-restauration.ts` (la mémoire
  écrite à chaque changement, la proposition de l'accueil).
  Aucune variable n'est mutée d'un module à l'autre : ce qui change de main vit
  dans l'objet `state` de `etat.ts` ;
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
sont `input_ops` dans `rdp-sidecar/src/entrees.rs`, `frame_msg` dans
`rdp-sidecar/src/trames.rs` et le
gestionnaire `ws.onmessage` / `send` dans `web/rdp.ts`.

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
| `14` | Clavier VNC | `keysym:u32`, `pressé:u8` (VNC seulement ; le RDP garde le `4`) |
| `16` | Recevoir les fichiers copiés sur le distant | JSON `{ "dossier"?: chemin }` (sinon le dossier des téléchargements) |
| `19` | Offrir des fichiers du poste au distant | JSON `[chemins absolus]` (fichiers ou dossiers) |

### Sidecar → application

| Code | Message      | Charge utile |
|------|--------------|--------------|
| `1`  | Connecté / nouvelle taille | `largeur:u16`, `hauteur:u16` |
| `2`  | Rectangle d'écran (FRAME) | `x:u16`, `y:u16`, `w:u16`, `h:u16`, puis pixels RGBA |
| `3`  | Erreur       | message UTF-8 — **réservé**, voir ci-dessous |
| `7`  | Statistiques | `fps:u16`, `débit:u32` (Ko/s), `latence:u16` (ms) |
| `8`  | Presse-papiers (distant → poste) | texte UTF-8 |
| `13` | Écran, plusieurs rectangles | `n:u8`, puis `n` fois (`x:u16`, `y:u16`, `w:u16`, `h:u16`, pixels RGBA) |
| `15` | Fichiers copiés sur le distant | JSON `{ dossier, octets, fichiers: [{ chemin, taille, dossier }] }` : la liste seulement, jamais le contenu |
| `17` | Progression d'une réception | JSON `{ fichier, fait, total, termines, nombre }` |
| `18` | Bilan d'une réception ou d'une offre | JSON `{ sens: "reception" \| "offre", dossier?, fichiers, octets, erreurs: [] }` |
| `20` | Bloc de son | `format:u8`, `ts:u32`, `cadence:u32`, `canaux:u8`, `bits:u8`, puis PCM 16 bits entrelacé (un mégaoctet au plus) |
| `21` | Volume demandé par le serveur | `gauche:u16`, `droit:u16` |

Le son passe par le canal statique RDPSND ([MS-RDPEA]) : `rdp-sidecar/src/son.rs`
n'annonce que du PCM 16 bits (44,1 et 48 kHz, stéréo puis mono), reçoit les
blocs d'ondes et les relaie tels quels ; `web/audio.ts` les joue par WebAudio
bout à bout sur un curseur de temps, et applique le volume. Le processus ne
touche à aucun périphérique audio. `--sans-son` (réglage de la palette) retire
le canal de la négociation.

Le dossier partagé passe par le canal statique RDPDR ([MS-RDPEFS]) :
`--lecteur <dossier>` fait annoncer un lecteur « Avash » (périphérique
`Filesystem`, annoncé une fois l'utilisateur connecté), et le serveur pilote
tout par des requêtes d'entrée-sortie que `rdp-sidecar/src/disque.rs` sert
depuis un fil dédié (créer, lire, écrire, énumérer, informations de fichier et
de volume, disposition, renommage, verrous), en répondant par `completion_id`
hors de la boucle de session. Chaque chemin du serveur est ramené sous la
racine avant toute ouverture. MS-RDPEFS veut `rdpdr` annoncé avec `rdpsnd` :
le son coupé, un canal audio muet (aucun format, sans ALIVE) reste annoncé.

Les fichiers passent par le canal CLIPRDR ([MS-RDPECLIP] 2.2.5 : liste
`FileGroupDescriptorW`, flux `FileContentsRequest` / `Response`, verrous), porté
par IronRDP ; `rdp-sidecar/src/fichiers.rs` découpe une réception en morceaux
d'un mégaoctet (quatre en vol), écrit chaque réponse à sa position dans un
`.part` promu une fois complet, et parcourt les dossiers offerts pour servir
les octets que le distant demande.

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
- **Le canal graphique (MS-RDPEGFX) s'apprend, il ne se devine pas.** Deux
  familles de serveurs coexistent : celles qui dessinent par les mises à jour
  classiques dès l'activation — Windows, xrdp — et celles qui ne dessinent que
  par le pipeline graphique — GNOME Remote Desktop. Se tromper coûte cher des
  deux côtés, et le piège n'est pas où on l'attend : **le seul fait d'accepter
  le canal fait taire un serveur Windows**, qui tient dès lors pour acquis que
  le client dessinera par là. Il ne suffit donc pas de retenir son annonce de
  capacités ; il faut refuser le canal lui-même. Le client refuse par défaut,
  observe, et si la session se termine sans qu'une seule image ait été affichée,
  se reconnecte en l'acceptant — puis l'inscrit dans
  `~/.config/avash/rdp_canal_graphique`, à côté des empreintes de certificats.
  La reconnexion ne se paie qu'une fois par serveur. `AVASH_EGFX=toujours` ou
  `jamais` tranche à la main.

  Aucun signe ne permettrait de décider à l'avance. La redirection de session,
  tentante — c'est par elle que GNOME Remote Desktop passe —, se retrouve aussi
  devant une ferme Windows, où l'accepter produirait un écran noir.
- **Trois codecs graphiques, pas H.264.** RemoteFX Progressive — en tuiles
  simples chez GNOME Remote Desktop, affiné par paliers de qualité chez
  Windows, avec des tuiles « en différence » dès qu'un contexte de codec est
  rouvert sur une surface qui a déjà servi —, ClearCodec avec ses trois
  sous-codecs (brut, NSCodec, RLEX ; NSCodec est porté dans
  `rdp-sidecar/vendor/ironrdp-graphics`, l'amont le laissait vide et Windows
  y met les icônes), et le non compressé.
  IronRDP fournit l'entropie et la transformée ; l'assemblage progressif est
  à nous (`rdp-sidecar/src/progressif.rs`) : l'état des tuiles vit avec la
  surface et survit à `DeleteEncodingContext`, les différences s'ajoutent aux
  coefficients gardés, et le palier d'affinage est réécrit d'après FreeRDP,
  la référence, là où IronRDP 0.9 s'écarte de la spécification (un seul
  ruban SRL et brut par composante sur les dix bandes, LL3 en brut, décalage
  base + progressif − 1). Le client annonce toutes les
  versions de capacités jusqu'à la 10.7 mais y pose `AVC_DISABLED` : décoder
  H.264 supposerait une dépendance à un décodeur vidéo, pour un gain nul sur des
  bureaux de travail. Sans ce drapeau, un serveur qui retiendrait l'une de ces
  versions enverrait de la vidéo illisible et l'écran resterait vide.

- **Le pipeline graphique est complet côté commandes** : surfaces, cache de
  surfaces, remplissages unis, recopies entre surfaces, rattachement à une
  sortie, accusés de trame. Le cache n'est pas un raffinement : Windows y
  puise plus de six cents fois pour une seule ouverture de session, et
  l'ignorer laisse un écran où tout ce qui se répète manque.
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
- **`--disable-gpu-compositing` sous Windows, en session distante seulement.**
  Symétrique du point précédent, pour un autre symptôme : avash affiché à
  travers un RDP (un poste piloté à distance, ou un avash Windows imbriqué dans
  un autre) montrait des carrés noirs fugaces là où WebView2 compose par le GPU
  — vignettes vidéo, canvas, aperçus d'onglet. La surface GPU est virtualisée
  par le protocole RDP : la tuile arrive parfois avant son contenu et se voit un
  instant en noir, d'autant plus longtemps que la session est imbriquée.
  `main()` interroge `GetSystemMetrics(SM_REMOTESESSION)` (FFI directe vers
  user32, aucune caisse ajoutée) et, en session distante, pose la composition
  logicielle via `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`. Sur un écran physique,
  une valeur héritée de l'environnement est **retirée**, jamais filtrée : WebView2
  la concatène à la ligne de commande de Chromium, et un `--remote-debugging-port`
  ou un `--renderer-cmd-prefix` planté là par un pied local prendrait la webview.
  Une exception, symétrique de `WEBKIT_INSPECTOR_SERVER` sous Linux : sous
  pilotage WebDriver (`TAURI_WEBVIEW_AUTOMATION=true`, posé par tauri-driver),
  la variable est celle qu'Edge WebDriver pose lui-même pour commander
  l'application, et l'on n'y touche pas — la retirer laissait chaque scénario
  Windows mourir sur « DevToolsActivePort file doesn't exist ». Décision pure
  et testée dans `action_webview2`. (Edge WebDriver ne lance de toute façon
  plus une application WebView2 depuis sa version 133 : la suite Windows passe
  par un serveur WebDriver embarqué, fonctionnalité `webdriver` d'avash-ui,
  compilée par la suite et jamais par la publication.)
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
| Commandes Tauri (un fichier par domaine) | `crates/avash-ui/src/commands/` |
| Lancement du sidecar RDP | `crates/avash-ui/src/rdp.rs` |
| Sidecar RDP — point d'entrée | `rdp-sidecar/src/main.rs` |
| Sidecar RDP — ligne de commande, disposition clavier, résolution | `rdp-sidecar/src/args.rs` |
| Sidecar RDP — connexion, négociation NLA/RDSTLS, redirections, coupures | `rdp-sidecar/src/connexion.rs` |
| Sidecar RDP — session établie (boucle, cadencement, redimensionnement, statistiques) | `rdp-sidecar/src/session.rs` |
| Sidecar RDP — confiance au serveur (TOFU), fichier des empreintes | `rdp-sidecar/src/empreintes.rs` |
| Sidecar RDP — canal local (jeton, origine du WebSocket) | `rdp-sidecar/src/acces_local.rs` |
| Sidecar RDP — entrées (souris, clavier, verrous) | `rdp-sidecar/src/entrees.rs` |
| Sidecar RDP — trames vers l'interface (zone sale, format binaire) | `rdp-sidecar/src/trames.rs` |
| Sidecar RDP — presse-papiers CLIPRDR | `rdp-sidecar/src/presse_papiers.rs` |
| Sidecar RDP — son RDPSND, et sa lecture WebAudio | `rdp-sidecar/src/son.rs`, `web/audio.ts` |
| Sidecar RDP — lecteur partagé RDPDR (dossier du poste) | `rdp-sidecar/src/disque.rs` |
| Sidecar RDP — capture `--shot` | `rdp-sidecar/src/capture.rs` |
| Canal graphique RDP (MS-RDPEGFX) | `rdp-sidecar/src/egfx.rs` |
| Codec RemoteFX Progressive | `rdp-sidecar/src/progressif.rs` |
| Surfaces et cache du canal graphique | `rdp-sidecar/src/surface.rs` |
| Magnétoscope (capture et rejeu) | `rdp-sidecar/src/magnetoscope.rs` |
| Front (cœur : arbre, onglets, terminaux, amorçage) | `web/main.ts` |
| Front (xterm.js et ses extensions, chargés à part au premier terminal) | `web/xterm-charge.ts` |
| Front (repère de mesure du démarrage, premier module évalué) | `web/mesure-demarrage.ts` |
| Front (un module par domaine : `etat`, `sftp`, `rdp`, `tunnels`, `snippets`, `dialogues`…) | `web/*.ts` |
| Front (logique pure testable) | `web/filters.ts` |
| Front (réglages persistants) | `web/prefs.ts` |
| Client SFTP (dont bandes parallèles) | `crates/avash/src/sftp.rs` |
| Port série | `crates/avash/src/serie.rs`, `crates/avash-ui/src/commands/serie.rs` |
| Diagnostic exporté | `crates/avash-ui/src/commands/diagnostic.rs` |
| Mémoire des onglets | `crates/avash-ui/src/commands/onglets.rs`, `web/onglets-memoire.ts`, `web/onglets-restauration.ts` |
| Vue partagée | `web/vue-partagee.ts` |
| Validation qualité | `check.sh`, `scripts/guard.sh` |
| Tests de bout en bout | `e2e/` |

## Correctifs portés sur IronRDP

`rdp-sidecar/vendor/` contient quatre crates d'IronRDP copiés, chacun avec un
ou deux changements ciblés. Ce n'est pas un fork de confort : ce sont des
défauts qui touchent tout client IronRDP parlant à xrdp, à GNOME Remote Desktop
ou à Windows, et qui rendaient avash inutilisable contre des serveurs légitimes.

| Crate | Défaut | Symptôme |
|---|---|---|
| `ironrdp-session` | le remplissage de fin de ligne n'est pas retiré dans les chemins compressés ; le PDU de redirection est rejeté ; deux paniques déclenchables par un serveur | image cisaillée en diagonale ; GNOME Remote Desktop ferme sans un mot ; le client tombe |
| `ironrdp-connector` | la mesure de bande passante n'est jamais renvoyée ; le drapeau du pipeline graphique n'est pas annoncé | connexion suspendue sans fin ; GNOME Remote Desktop refuse la connexion |
| `ironrdp-pdu` | ClearCodec lit deux champs dans l'ordre inverse de la spécification ; le RLEX à une seule couleur est lu sans son octet compacté | aucun bureau Windows ne s'affiche par le canal graphique ; les coins unis de la barre des tâches sont refusés |
| `ironrdp-graphics` | le sous-codec NSCodec de ClearCodec est un bras vide, sans erreur | les icônes de la barre des tâches sont des carrés noirs, mis en cache puis retamponnés |

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
