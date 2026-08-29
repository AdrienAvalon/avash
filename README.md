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

**143 tests verts** (105 Rust, 38 TypeScript) · clippy strict · `cargo audit`
sans vulnérabilité non justifiée · démarrage 0,17 s.

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

## Feuille de route
- v0.1 (ce soir/aujourd'hui) : CLI + parseur + connexion russh → **GUI dès webkit installé**
- v0.2 : ~~tunnels~~ ✅, ~~SFTP glisser-déposer~~ ✅, snippets
- v0.3 : chiffrement secrets, imports, recherche instantanée
- v1 : RDP + multi-exécution + santé hôtes