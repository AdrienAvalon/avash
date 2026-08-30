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
    validation des champs contre l'injection ;
  - `ssh.rs` — connexion et authentification (russh), vérification TOFU des
    clés d'hôte, redirections de ports ;
  - `sftp.rs` — client SFTP ;
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
  RDP (letterbox). Couverte par `filters.test.ts` (Vitest) ;
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

Le sidecar `avash-rdp` écoute sur `127.0.0.1:<port éphémère>` et n'accepte
qu'**une** connexion, authentifiée par un jeton aléatoire. Il l'annonce sur sa
sortie standard sous la forme `<port> <jeton>` ; l'application lit ces valeurs
(voir `crates/avash-ui/src/rdp.rs`) et ouvre le WebSocket. La **première trame**
envoyée par le front est le jeton (texte) ; toute connexion qui ne le présente
pas est rejetée (voir `rdp-sidecar/src/main.rs`).

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

### Sidecar → application

| Code | Message      | Charge utile |
|------|--------------|--------------|
| `1`  | Connecté / nouvelle taille | `largeur:u16`, `hauteur:u16` |
| `2`  | Rectangle d'écran (FRAME) | `x:u16`, `y:u16`, `w:u16`, `h:u16`, puis pixels RGBA |
| `3`  | Erreur       | message UTF-8 |
| `7`  | Statistiques | `fps:u16`, `débit:u32` (Ko/s), `latence:u16` (ms) |
| `8`  | Presse-papiers (distant → poste) | texte UTF-8 |

Le code `6` (ACK de rendu) implémente un **cadencement adaptatif** : le front
accuse réception de chaque frame, et le sidecar n'envoie la suivante qu'une fois
l'ACK reçu (avec un `ACK_TIMEOUT` de garde), ce qui évite d'inonder un poste
plus lent que le débit du serveur.

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
- **Vérification des clés d'hôte TOFU** stricte : refus si la clé change.
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
| Validation qualité | `check.sh`, `scripts/guard.sh` |
| Tests de bout en bout | `e2e/` |
