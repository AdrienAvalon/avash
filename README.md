# Avash 😼 — le gestionnaire de connexions d'Ava

**Nom officiel validé par Adrien le 28/08 06:56.** *Ava + sh = avash.* Aussi : le ronronnement du chat heureux — ton terminal, content.

## Vision
Gestionnaire graphique de connexions : PuTTY/MobaXterm en mieux — **beau, simple, ultra rapide, sécurisé, multi-plateforme, révolutionnaire**.
- Natif (Tauri 2, pas d'Electron) : ~15 Mo, <100 Mo RAM, démarrage <1 s
- Zéro config : lit `~/.ssh/config` nativement
- Secrets chiffrés, aucune télémétrie
- Protocoles : SSH, SFTP, RDP (IronRDP), VNC, série, mosh (phase 2)
- Tueuses : multi-exécution, tunnels visuels, ProxyJump chaîné cliquable, édition distante, snippets, santé hôtes + WoL, enregistrement asciinema, import PuTTY/Moba
- DSL d'hôtes versionnable Git (v0.3+)

## Stack
- **Tauri 2** + xterm.js + russh (SSH pur Rust) + IronRDP
- Locale : `/home/avalon/dev/avash`

## État (29/08 — tunnels SSH)

**162 tests verts** (117 Rust, 45 TypeScript) · clippy strict · `cargo audit`
sans vulnérabilité non justifiée · démarrage 0,17 s.

### Organisation des hôtes — tags (29/08, soir)

Étiquetage des hôtes via `#Tags: prod, web` dans le bloc `~/.ssh/config`
(reste un commentaire pour OpenSSH, round-trip garanti). Barre de filtres par
tag sous « Hôtes », puces cliquables sur chaque hôte, recherche qui matche
aussi les tags. Champ Tags dans l'édition d'hôte.

### Agent SSH (29/08, soir)

Authentification via l'agent (`ssh-agent`, `gpg-agent`, Pageant/pipe sous
Windows). Ordre, comme OpenSSH : clé de l'hôte → **agent** → mot de passe.
Une clé déverrouillée une fois, ou sur token matériel (YubiKey), évite toute
saisie. L'interface ne réclame plus de mot de passe quand l'agent a des
identités. Vérifié bout-en-bout avec un vrai `ssh-agent` (connexion réussie
sans clé ni mot de passe fournis à Avash).

### Refonte « pro » (29/08, soir)

Quatre axes, chacun vérifié dans l'application réelle :

1. **Icônes SVG** — tous les emoji du châssis remplacés par un jeu SVG
   cohérent (`web/icons.ts`, trait fin, grille 24, `currentColor`). Mascotte
   😼 conservée comme marque. Cross-plateforme, gratuit en perf.
2. **Thème clair + suivi système** — palette claire complète (chrome ET
   terminal), bascule système/clair/sombre persistée dans le bandeau, réaction
   au thème de l'OS.
3. **Barre de titre intégrée** — `decorations: false`, barre custom avec titre
   dynamique (« <hôte> — Avash »), contrôles min/max/close thémés, poignées de
   redimensionnement maison (Wayland ne les fournit plus sans décorations).
4. **Performance** — idle sur l'accueil ramené de ~11 % à ~7 % CPU (le
   bobbing perpétuel de la mascotte, remplacé par une entrée unique) ;
   animations gelées quand la fenêtre perd le focus ; polling des tunnels
   coupé quand il n'y en a aucun. RSS ~454 Mo (plancher WebKitGTK, inhérent
   à la webview).

Reste à valider sur Windows : redimensionnement/contrôles de la barre de titre.

### Audit complet + correctifs (29/08)

Double relecture (cœur Rust orienté sécurité, front). Corrigés :

- **[grave] Injection de directive SSH** : `HostName`/`User`/`IdentityFile`
  n'étaient pas validés à l'écriture dans `~/.ssh/config` (seul l'alias
  l'était). Un `\n` injectait une directive arbitraire — dont `ProxyCommand`,
  exécuté par `ssh` à la connexion (exécution de commande). Les trois champs
  sont désormais validés (`validate_host`). Test de non-régression.
- **[correction] « Mémoriser le mot de passe » cassé sans `User`** : le front
  envoyait `user: null` à des commandes attendant `String`, et la clé du
  trousseau ne correspondait pas à celle de relecture. `user` devient
  optionnel et résout l'utilisateur courant, comme `from_alias`. Vérifié.
- **[correction] `pty-closed` fantôme** : après rechargement de fenêtre, la
  session évincée fermait le nouvel onglet réutilisant son id. Chaque session
  porte un `epoch` ; l'événement n'est plus émis si l'id porte une session
  plus récente.
- **[ressource] Course SFTP** : deux commandes concurrentes ouvraient deux
  connexions ; la perdante fuyait sans `close()`. Vérification atomique sous
  verrou, fermeture du handle en trop.
- **[compat] Clés RSA** : présentées en SHA-1 (refusé par OpenSSH récent) →
  `rsa-sha2-256`. Vérifié : connexion RSA réelle réussie.
- **[front] Injection HTML** via un nom de variable de snippet (`innerHTML`) →
  construction par le DOM. Palette couverte par le garde des raccourcis.
  Retour visible si un téléchargement SFTP est lancé pendant un transfert.

Restés documentés, non corrigés (faible gravité) : fenêtre de course sur un
tunnel `-R` à port 0 (premières connexions), commentaires de `~/.ssh/config`
entre un bloc supprimé et le suivant absorbés.

### Snippets (29/08)

Des commandes réutilisables, envoyées dans le terminal en un clic.

- **Variables `{{nom}}`** : demandées à l'envoi, avec aperçu en direct de la
  commande rendue.
- **Multi-exécution** : envoi sur plusieurs sessions ouvertes à la fois
  (cases à cocher, l'active pré-cochée) — pratique pour une flotte.
- **Exécuter** (avec Entrée) ou **insérer** (relire avant de valider) ;
  multi-lignes gérées (`\n` → `\r`, une commande par ligne).
- Persistés dans `~/.config/avash/snippets.yaml` (écriture atomique).
- Logique testée des deux côtés (extraction/rendu/charge terminal en Rust,
  helpers front) ; multi-exécution vérifiée contre `sshd` réel (le snippet
  crée bien un fichier sur les deux sessions).

### Ajustements UX (29/08, après-midi)

- **Bouton *Fichiers*** dans la barre d'onglets : ouvre/ferme le panneau SFTP
  à la demande (état reflété, désactivé sans session ; Ctrl+B garde son rôle).
- **Simple clic = sélection** (hôtes et fichiers, surlignés), **double-clic =
  action** (se connecter / ouvrir un dossier / télécharger). Plus de connexion
  ni de navigation par mégarde.
- **SFTP : plus d'erreur à l'ouverture.** Le dossier de départ `.` est résolu
  en chemin absolu (`sftp_realpath` → `canonicalize`) avant d'être listé —
  certains serveurs refusent `read_dir(".")`. La barre affiche le vrai chemin.

### SFTP complet (29/08, midi)

- **Glisser-déposer** depuis le bureau : le panneau s'ouvre de lui-même et
  envoie les fichiers dans le dossier courant (événement drag-drop de Tauri,
  chemins natifs — pas de lecture du fichier par la webview).
- Sélecteur natif (*Envoyer…*, plugin `dialog`), **progression** en direct
  (barre + octets, événement `sftp-progress` borné à 12/s), blocs de 64 Kio.
- Chemin **éditable** (Entrée), dossier parent, rafraîchir, **par onglet**.
- Menu contextuel : télécharger, *aller ici dans le terminal* (`cd` cité pour
  le shell), copier le chemin, renommer, nouveau dossier, supprimer (dossier
  vide seulement — pas de `rm -rf` implicite).
- Icônes par type, tailles, dates courtes.
- Le bouton *Envoyer* n'avait **aucun** gestionnaire avant cette passe.
- Vérifié contre `sshd` réel : envoi de 5 Mo octet pour octet identique.

### Logo de la distribution (29/08, après-midi)

À chaque ouverture de session, une sonde `cat /etc/os-release || uname -s ||
ver` part sur un canal exec séparé (bornée à 4 s, sans retarder le terminal).
Le front en tire le logo **Font Logos** de la Nerd Font déjà embarquée — aucune
image — et la couleur de marque, mémorisés par hôte dans `localStorage` pour
s'afficher dès le lancement suivant. Dérivées inconnues → famille (`ID_LIKE`,
ex. CachyOS → Arch) → Tux. Vérifié dans l'application contre le `sshd` local.

### Refonte visuelle (29/08, après-midi)

Audit par captures de chaque état dans l'application réelle, puis réécriture
du bloc de style en un seul système de jetons (3 profondeurs de fond, un
accent, rayons/ombres/durées nommés). Sans bibliothèque, CSS seul.

- **Bugs trouvés par l'audit** : `hidden` vaincu par `display:flex` (les
  champs mot de passe, clé **et** alias s'affichaient tous en même temps
  dans « Connexion directe ») ; le bouton *Enregistrer* du formulaire
  Tunnels rogné (bloc `<details>` comprimé par la modale en `flex-column`) ;
  « Aucune session » restait affiché à côté des onglets.
- Barre latérale : avatar d'hôte aux initiales (teinte stable par nom),
  pastille **uniquement** quand une session est ouverte dessus (l'ancien point
  vert permanent ne voulait rien dire), en-tête de section, icône de
  recherche SVG + rappel `Ctrl K`, actions en liste.
- Onglets : indicateur souligné, croix révélée au survol.
- Formulaires : interrupteur segmenté à la place des radios natives,
  `<select>` et champs nombre sans chrome système, `accent-color`.
- Performance : transitions ≤ 160 ms sur `background`/`color`/`transform`
  seulement (pas d'ombre animée), flou de fond limité à 3 px,
  `prefers-reduced-motion` respecté.

### Reconnexion et mot de passe mémorisé (29/08, après-midi)

- **Bug corrigé** : un mot de passe coché « mémoriser » était bien écrit dans
  le trousseau, mais `pty_open` l'écrasait ensuite par la saisie vide du
  front (`target.password = None`) → il fallait le retaper à chaque fois.
  Trouvé en vérifiant d'abord que le trousseau relit bien l'entrée
  (`examples/keyring_persist.rs`, d'un processus à l'autre) avant de
  soupçonner le code.
- **Session terminée** (exit, coupure) : **Entrée** reconnecte dans le même
  onglet, **Ctrl+W** le ferme. Vaut aussi pour une connexion échouée. Vérifié
  dans l'application réelle contre le `sshd` local.

### Tunnels SSH (nouveau)

Les trois redirections d'OpenSSH, visuelles : **`-L`** (local → serveur →
destination), **`-R`** (serveur → local → destination) et **`-D`** (mandataire
SOCKS5 sortant par le serveur). Menu contextuel d'un hôte → *Tunnels…*, ou
bouton *Tunnels* de la barre latérale.

- Chaque tunnel vit sur **sa propre connexion SSH** : fermer un onglet ne le
  coupe pas, et inversement. Keepalive 30 s (3 échecs → tunnel marqué
  « connexion perdue », bouton *Relancer*).
- Compteurs en direct : connexions en cours, octets ↑/↓ — mis à jour pendant
  la connexion, pas seulement à sa fin (une session VNC ou Postgres dure).
- Définitions dans `~/.config/avash/tunnels.yaml` (écriture atomique).
- Écoute **loopback uniquement** ; un `-R` ne relaie que vers la destination
  déclarée (un serveur malveillant ne peut pas faire ouvrir une connexion
  locale arbitraire). SOCKS : CONNECT seul, sans authentification, SOCKS4
  refusé.
- Vérifié contre **OpenSSH 10.5 réel** avec `examples/tunnel_probe.rs` : les
  trois types relaient, le port `-R` est libéré à la fermeture.

### Bugs de correction trouvés pendant l'audit

| Défaut | Conséquence | Détecté par |
|---|---|---|
| `run()` cassait sur `Eof` | **code de sortie toujours 0** — un déploiement de clé raté passait pour réussi | test contre un vrai sshd |
| serveur de test : `exit-status` avant `eof` | masquait le bug ci-dessus | audit de l'ordre réel |
| `Match` non reconnu | directives attribuées au mauvais hôte — mauvais user/port silencieux | relecture du parseur |
| `pty_close` : `into_inner().unwrap()` | plantait à la fermeture si un mutex était empoisonné | audit des paniques |

### Chemins de sécurité, chacun couvert par un test

- Clé d'hôte modifiée refusée (vérifié comme échouant sur le code d'avant)
- Injection shell dans le déploiement de clé refusée
- Injection de directive via un alias `~/.ssh/config` refusée
- Traversée de chemin au téléchargement et dans un nom de clé refusée
- Traversée SFTP (`..`) impossible via `parentDir`
- Mot de passe absent des traces (`Debug` masquant, testé)
- `Match` ne contamine plus l'hôte précédent
- `Include` circulaire borné à 16 niveaux
- Code de sortie fiable (non-régression)

**Chemin non testé, assumé** : le refus d'un certificat d'hôte SSH. Simuler
un serveur à certificat signé demande une infrastructure lourde ; le chemin
est en échec sécurisé (il refuse), et le défaut de `russh` refuse aussi.

### Vérifié contre un vrai serveur, pas un simulacre

Les bugs les plus pénibles (terminal muet, code de sortie, ordre des messages
SSH) ne se voyaient qu'en conditions réelles. `examples/pty_probe.rs`,
`examples/tunnel_probe.rs` et `examples/keyring_check.rs` sont conservés comme outils : ils testent contre le
`sshd` et le trousseau réels de la machine, là où un simulacre ment.

### Objectifs de la spec, mesurés

| | Visé | Mesuré | |
|---|---|---|---|
| Démarrage | < 1 s | 0,17 s | ✅ |
| RAM | < 100 Mo | 297 Mo (PSS) | ❌ plancher WebKit |

L'objectif de 100 Mo n'est pas atteignable avec une webview ; voir plus bas.

## Distribution

**Un fichier par système**, déployable par copie. Voir **`RELEASE.md`** pour la
procédure complète (build, vérification, signature, faux positifs AV).

- **Linux** : `Avash_<version>_amd64.AppImage` — un seul fichier autonome
  (embarque WebKitGTK), `chmod +x` et lancer. Construit et vérifié ici.
- **Windows** : installeur NSIS `.exe` (à construire sur Windows ; dépend de
  WebView2, préinstallé Win10 récent/Win11). Signature Authenticode câblée,
  certificat à fournir.
- `./scripts/release.sh [--sign-gpg <KEYID>]` : valide, construit, produit
  `SHA256SUMS` (+ signature GPG). Aucun packer, métadonnées complètes : la
  surface de faux positif antivirus est réduite au minimum contrôlable.

## Feuille de route
- v0.1 (ce soir/aujourd'hui) : CLI + parseur + connexion russh → **GUI dès webkit installé**
- v0.2 : ~~tunnels~~ ✅, ~~SFTP glisser-déposer~~ ✅, snippets
- v0.3 : chiffrement secrets, imports, recherche instantanée
- v1 : RDP + multi-exécution + santé hôtes