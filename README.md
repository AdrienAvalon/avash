<div align="center">

<img src="web/public/hero.svg" alt="avash" width="200">

# avash

**Gestionnaire graphique de connexions SSH et RDP — natif, rapide, sécurisé.**

[![Licence: AGPL v3](https://img.shields.io/badge/licence-AGPL--3.0-blue.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-0.6.2-8b7cf6.svg)](CHANGELOG.md)
[![Tests](https://img.shields.io/badge/tests-869%20verts-brightgreen.svg)](#qualité)

</div>

---

## Qu'est-ce que c'est

avash réunit vos connexions SSH, vos bureaux RDP et vos transferts de fichiers
dans une seule application native. Il lit directement votre `~/.ssh/config` —
aucune migration, aucun format propriétaire : les hôtes déjà déclarés
apparaissent au premier lancement, et les modifications faites depuis avash
restent lisibles par `ssh` en ligne de commande.

Construit avec Tauri 2 et Rust (pas d'Electron) : l'application pèse une
vingtaine de mégaoctets et démarre en une fraction de seconde.

## Fonctionnalités

**Connexions**
- **SSH** — terminal complet (xterm.js), plusieurs sessions en onglets, `ProxyJump` en chaîne
- **RDP** — bureaux distants intégrés (IronRDP), redimensionnement natif : le bureau distant s'adapte réellement à la fenêtre, sans zoom d'image
- **Windows, xrdp et GNOME Remote Desktop** — y compris les serveurs qui redirigent la connexion vers une autre session (RDSTLS) et ceux qui ne dessinent que par le canal graphique (MS-RDPEGFX : ClearCodec, RemoteFX Progressive, cache de surfaces)
- **SFTP** — panneau de fichiers distants : parcourir, envoyer, télécharger, renommer, supprimer
- **Tunnels SSH** — locaux (`-L`), distants (`-R`) et SOCKS (`-D`), avec leur état en direct

**Organisation**
- Arborescence de dossiers pour ranger hôtes SSH et bureaux RDP ensemble, par glisser-déposer
- Étiquettes, recherche instantanée et palette de commandes (`Ctrl+K`)
- Snippets : commandes réutilisables avec variables, envoyables sur plusieurs sessions
- Interface en **français** et en **anglais** : suit la locale, bascule dans la palette mémorisée, ou `AVASH_LANGUE=fr|en` dans l'environnement
- **Santé des hôtes** : une sonde TCP par hôte depuis la palette, ou au démarrage sur option, voyant vert ou rouge sur chaque ligne mémorisé d'un lancement à l'autre
- **Enregistrement de session** au format asciicast v2 (menu du terminal), rejouable avec `asciinema play` ; l'écran tel qu'il est au départ, la sortie ensuite, jamais les frappes ; la liste des enregistrements dans la palette
- Import des sessions **PuTTY** (fichiers ou registre) et **MobaXterm** (`MobaXterm.ini`, `.mxtsessions`), bureaux RDP et dossiers compris, doublons signalés, clés `.ppk` converties par `puttygen` s'il est là
- **Utilisable au clavier de bout en bout** — la barre latérale se parcourt aux flèches, `Entrée` ouvre, `Maj+F10` donne le menu

**Sécurité**
- Mots de passe conservés uniquement dans le **trousseau du système** — jamais en clair sur le disque, et jamais transmis à l'interface : le cœur natif les lit au moment de connecter
- Vérification des clés d'hôte SSH (TOFU) : connexion refusée si la clé change, **y compris quand seul l'algorithme diffère** — un cas où l'aide fournie par notre bibliothèque SSH répondait « hôte inconnu », donc « premier contact »
- Vérification du serveur RDP par la même règle, **avant** que CredSSP ne transmette le moindre identifiant. Le repli de NLA vers TLS seul est refusé : un serveur ne peut pas nous faire livrer un mot de passe sans authentification mutuelle
- Mot de passe RDP transmis au processus RDP par entrée standard, jamais en argument de commande — invisible dans la liste des processus
- Presse-papiers partagé avec les bureaux distants **seulement si vous le voulez**, dans les deux sens, révocable à tout moment (`Ctrl+K`)
- `~/.ssh/config`, `known_hosts` et les fichiers de configuration sont écrits atomiquement : une coupure ne peut pas les laisser vides
- Aucune télémétrie, aucun appel réseau autre que vos connexions

Le modèle de sécurité, ce qu'il couvre et ce qu'il ne couvre pas, est détaillé
dans [SECURITY.md](SECURITY.md).

## Installation

### Linux (AppImage)

```bash
chmod +x Avash_0.6.2_amd64.AppImage
./Avash_0.6.2_amd64.AppImage
```

### Windows

Deux formes au choix :

- **Installeur** (`Avash_x.y.z_x64-setup.exe`) — installation classique.
- **Version portable** (`avash-x.y.z-windows-x64.zip`) — à décompresser et
  lancer, sans installation ni écriture dans la base de registre. Garder
  `avash-rdp.exe` à côté d'`avash.exe` : c'est le processus qui assure le RDP.

Windows affiche un avertissement au lancement de l'installeur : **avash n'est pas
signé numériquement**. C'est le comportement normal pour un logiciel sans
certificat de signature de code (Authenticode) — cliquer sur « Informations
complémentaires » puis « Exécuter quand même ».

### macOS (image disque)

`Avash_x.y.z_aarch64.dmg`, pour les Mac à puce Apple. L'application n'est pas
notarisée : au premier lancement, Gatekeeper refuse d'ouvrir un logiciel « d'un
développeur non identifié ». Clic droit sur l'application → **Ouvrir**, une
fois ; ou, dans un terminal :

```bash
xattr -d com.apple.quarantine /Applications/Avash.app
```

La version macOS est construite et testée par la chaîne d'intégration
continue (cœur, processus RDP, interface), mais **n'a pas encore été éprouvée
sur une machine réelle** : les retours sont bienvenus.

Deux moyens de vérifier qu'un fichier téléchargé est bien le nôtre :

```bash
# 1. Empreinte : compare avec le fichier SHA256SUMS publié avec la version
sha256sum Avash_0.6.2_x64-setup.exe

# 2. Provenance : preuve cryptographique que le binaire vient de ce dépôt,
#    de ce commit, produit par notre chaîne d'intégration continue
gh attestation verify Avash_0.6.2_x64-setup.exe --repo AdrienAvalon/avash
```

La seconde vérification est plus forte que la première : elle ne dit pas
seulement que le fichier est intact, mais **d'où il vient**.

### Compiler depuis les sources

Prérequis : Rust stable, Node.js 22+, et les dépendances système de Tauri.

```bash
# Debian / Ubuntu
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev \
    libayatana-appindicator3-dev librsvg2-dev patchelf

git clone https://github.com/AdrienAvalon/avash.git avash && cd avash

# 1. Le front : le binaire l'embarque
(cd web && npm install && npx vite build)

# 2. Le processus RDP : avash-ui le déclare en ressource embarquée, son binaire
#    doit exister avant la compilation (le dossier binaires/ n'est pas versionné)
cargo build --release --manifest-path rdp-sidecar/Cargo.toml
mkdir -p crates/avash-ui/binaries
cp rdp-sidecar/target/release/avash-rdp \
   crates/avash-ui/binaries/avash-rdp-x86_64-unknown-linux-gnu

# 3. L'application
cargo build --release -p avash-ui
./target/release/avash-ui
```

Le script `./scripts/release.sh` enchaîne ces étapes et produit l'AppImage.

## Raccourcis

| Raccourci | Action |
|---|---|
| `Ctrl+K` | Palette de commandes |
| `Ctrl+W` | Fermer l'onglet |
| `Ctrl+Tab` | Onglet suivant |
| `Ctrl+1`…`9` | Aller à un onglet |
| `Ctrl+B` | Panneau de fichiers (SFTP) |

Dans la barre latérale, une seule tabulation suffit pour y entrer ; ensuite :

| Touche | Action |
|---|---|
| `↑` `↓` | Hôte ou dossier précédent / suivant |
| `Origine` `Fin` | Première / dernière ligne |
| `Entrée` | Se connecter, ou plier un dossier |
| `Maj+F10` | Menu contextuel — qui se parcourt aussi aux flèches |
| `Échap` | Refermer le menu, en rendant le focus à la ligne |

## Qualité

**869 tests** couvrent le projet, tous exécutés à chaque commit :

| Niveau | Nombre | Ce qui est vérifié |
|---|---|---|
| Cœur (`crates/avash`) | 145 | parseur `~/.ssh/config` et son **fuzzing par mutation** (plus cinq cibles cargo-fuzz dans `fuzz/`), import PuTTY et MobaXterm, enregistrement asciicast, sonde de santé, clés d'hôte, secrets, dossiers, tunnels, snippets, écritures atomiques, clés générées privées dès leur création |
| Intégration | 33 | contre un **vrai serveur SSH** : authentification et ses refus, PTY, SFTP sur la session du terminal, tunnels, rebonds `ProxyJump` ; l'outil en ligne de commande exercé comme binaire |
| Interface (`crates/avash-ui`) | 62 | commandes Tauri, import de sessions, enregistrement, santé des hôtes, magasin de sessions sur moteur factice (annulation pendant la connexion, éviction par époque), résolution des rebonds `ProxyJump`, décodage UTF-8 en flux, verrous clavier, annonce du processus RDP, variables d'environnement de la webview |
| Processus RDP | 86 | empreinte du serveur, fichier des empreintes, écriture atomique, plafond de résolution, négociation, identifiants et domaine, format binaire des trames, configuration après redirection, origine WebSocket, disposition clavier, isolation des tests, zone sale, **résistance aux messages malformés**, canal graphique (surfaces, cache, ClearCodec, RemoteFX Progressive), magnétoscope, rejeu d'enregistrements réels, fuzzing par mutation |
| Paquets IronRDP portés | 387 | nos correctifs — remplissage des tuiles, bande passante, redirection de serveur, capacités précoces, **ordre des champs de ClearCodec** — et les tests amont de `ironrdp-pdu`, qui ne s'exécutaient nulle part (voir [rdp-sidecar/vendor](rdp-sidecar/vendor/README.md)) |
| Front (Vitest) | 104 | logique pure : arborescence, chemins de dossiers, filtres, scancodes, mappage souris, réglages, collage sûr, traductions (couverture des deux dictionnaires, variables, page) |
| Bout en bout (WebdriverIO) | 52 | l'application réelle : connexion SSH et RDP effectives, SFTP, enregistrement asciicast, santé des hôtes, presse-papiers RDP, dossiers, import PuTTY, langue, modales, tunnels, snippets, accessibilité, navigation au clavier, **audit axe-core sur les deux thèmes** — tous en intégration continue, serveurs locaux compris |

S'y ajoutent `clippy` en mode strict — **en profil debug et en profil release**,
qui ne voient pas le même code — ESLint typé, stylelint, knip (code mort),
`cargo audit`, `cargo deny` et `npm audit` sur tous les arbres de dépendances,
et une garde qui interdit les motifs dangereux. Sur le dépôt : CodeQL,
gitleaks, le Scorecard de l'OpenSSF et Dependabot (voir
[CONTRIBUTING.md](CONTRIBUTING.md)).

### Accessibilité : un juge extérieur

Les vérifications écrites à la main couvrent ce à quoi on a pensé — rôles des
modales, piège à focus, retour du focus. `axe-core` couvre ce à quoi on n'a pas
pensé, et il a trouvé du premier coup : un texte secondaire à **3,15:1** au lieu
de 4,5, des initiales d'avatar à 4,44, un champ sans étiquette visible, un rôle
ARIA interdit sur un `<form>`. Le thème clair était **pire encore** — 2,45:1 —
et aucun test ne l'aurait montré : ils tournent tous en sombre.

Corrigé par le calcul, pas à l'œil : chaque couleur retenue tient 4,5:1 sur
*toutes* les surfaces où elle apparaît, et l'encre des initiales mêle la teinte
de l'hôte à la couleur de texte du thème, de sorte que la lisibilité suive
automatiquement.

### Rejouer un serveur disparu

Le dialogue d'un vrai serveur est capturé une fois, puis rejoué sans réseau :
**5 millisecondes contre 5 secondes de connexion**. Une machine du parc devient
une fixture permanente, et le rendu obtenu est comparé à une empreinte de
référence — en débranchant le correctif du cisaillement, elle change.

Surtout, ces enregistrements servent de graines à un **fuzzing par mutation**.
Muter des octets au hasard ne franchit jamais les premières validations ; muter
du trafic authentique atteint le décodeur d'images. Il y a trouvé deux façons
pour un serveur hostile de faire tomber le client — une écriture hors tampon et
un débordement arithmétique — l'une et l'autre corrigées. Détails dans
[SECURITY.md](SECURITY.md).

### Conformité RDP : de vrais serveurs, pas des simulacres

Trois défauts RDP corrigés en 0.3.3 — image cisaillée en diagonale, clavier
interprété en QWERTY, connexion suspendue sans fin — ont **tous** été signalés
par l'usage, et **aucun** n'était visible depuis les tests. Les tests unitaires
vérifiaient nos fonctions, la suite bout en bout vérifiait l'interface ; entre
les deux se trouvait le seul endroit où ces défauts vivaient : le dialogue réel
avec un serveur RDP.

Un parc de serveurs en conteneur comble ce vide : deux bureaux xrdp — XFCE et
GNOME, parce qu'ils ne dessinent pas de la même façon — et un sshd qui refuse la
méthode `password`, pour éprouver le repli `keyboard-interactive` dont l'absence
empêchait tout compte de domaine de se connecter.

```bash
scripts/parc-rdp.sh up tous        # XFCE 3390, GNOME 3391, sshd 2222
scripts/conformite.sh tous         # connexion, image, trafic, clavier, SSH, SFTP
scripts/parc-rdp.sh down
```

Le détecteur de cisaillement est lui-même éprouvé : en désactivant le correctif
porté, il annonce `CISAILLÉE décalage=-2 (96% des lignes)` ; correctif remis,
`saine décalage=+0 (100%)`. Détails dans
[tests-parc/README.md](tests-parc/README.md).

Une règle tient lieu de discipline : **un nouveau test doit avoir été vu
échouer**. On débranche ce qu'il couvre et on vérifie qu'il tombe — un test qui
ne peut pas échouer ne protège rien.

```bash
./check.sh              # tout valider
./check.sh --quick      # sans le build release
cd e2e && npm test      # tests bout en bout (ouvre des fenêtres)

# conformité RDP contre de vrais serveurs xrdp
scripts/parc-rdp.sh up tous && CONFORMITE_RDP=1 PARC=tous ./check.sh
```

## Architecture

Trois composants : un cœur SSH réutilisable (`crates/avash`), l'application
Tauri (`crates/avash-ui`), et un processus RDP séparé (`rdp-sidecar`) qui
communique par WebSocket binaire local. Détails dans
[docs/architecture.md](docs/architecture.md).

## Documentation

- [docs/feuille-de-route.md](docs/feuille-de-route.md) — le cap, les priorités et les règles de travail
- [CHANGELOG.md](CHANGELOG.md) — historique des versions
- [CONTRIBUTING.md](CONTRIBUTING.md) — développer et contribuer
- [SECURITY.md](SECURITY.md) — signaler une vulnérabilité, modèle de sécurité
- [docs/architecture.md](docs/architecture.md) — architecture technique
- [docs/journal-de-bord.md](docs/journal-de-bord.md) — journal de développement (archive)
- [tests-parc/README.md](tests-parc/README.md) — parc RDP local et conformité
- [rdp-sidecar/vendor/README.md](rdp-sidecar/vendor/README.md) — correctifs portés sur IronRDP

## Licence

avash est distribué sous licence **[AGPL-3.0-or-later](LICENSE)**.

Vous pouvez l'utiliser, l'étudier, le modifier et le redistribuer librement. En
contrepartie, toute version modifiée — y compris **mise à disposition comme
service en réseau** — doit être publiée sous la même licence.

**Licence commerciale.** Pour intégrer avash dans un produit propriétaire ou
l'exploiter comme service sans publier vos modifications, une licence
commerciale distincte est disponible. Contact : adrien.cros@outlook.com

© 2026 Adrien Cros. Tous droits réservés.
