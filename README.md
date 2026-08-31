<div align="center">

<img src="web/public/hero.svg" alt="avash" width="200">

# avash

**Gestionnaire graphique de connexions SSH et RDP — natif, rapide, sécurisé.**

[![Licence: AGPL v3](https://img.shields.io/badge/licence-AGPL--3.0-blue.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-0.3.1-8b7cf6.svg)](CHANGELOG.md)
[![Tests](https://img.shields.io/badge/tests-295%20verts-brightgreen.svg)](#qualité)

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
- **SFTP** — panneau de fichiers distants : parcourir, envoyer, télécharger, renommer, supprimer
- **Tunnels SSH** — locaux (`-L`), distants (`-R`) et SOCKS (`-D`), avec leur état en direct

**Organisation**
- Arborescence de dossiers pour ranger hôtes SSH et bureaux RDP ensemble, par glisser-déposer
- Étiquettes, recherche instantanée et palette de commandes (`Ctrl+K`)
- Snippets : commandes réutilisables avec variables, envoyables sur plusieurs sessions
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
chmod +x Avash_0.3.1_amd64.AppImage
./Avash_0.3.1_amd64.AppImage
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

Deux moyens de vérifier qu'un fichier téléchargé est bien le nôtre :

```bash
# 1. Empreinte : compare avec le fichier SHA256SUMS publié avec la version
sha256sum Avash_0.3.1_x64-setup.exe

# 2. Provenance : preuve cryptographique que le binaire vient de ce dépôt,
#    de ce commit, produit par notre chaîne d'intégration continue
gh attestation verify Avash_0.3.1_x64-setup.exe --repo AdrienAvalon/avash
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

**295 tests** couvrent le projet, tous exécutés à chaque commit :

| Niveau | Nombre | Ce qui est vérifié |
|---|---|---|
| Cœur (`crates/avash`) | 115 | parseur `~/.ssh/config`, clés d'hôte, secrets, dossiers, tunnels, snippets, écritures atomiques |
| Intégration | 24 | contre un **vrai serveur SSH** : authentification et ses refus, PTY, SFTP, tunnels, rebonds `ProxyJump` |
| Interface (`crates/avash-ui`) | 34 | commandes Tauri, décodage UTF-8 en flux, verrous clavier |
| Processus RDP | 9 | empreinte du serveur, fichier des empreintes, plafond de résolution |
| Front (Vitest) | 78 | logique pure : arborescence, filtres, scancodes, mappage souris, réglages |
| Bout en bout (WebdriverIO) | 35 | l'application réelle : connexion SSH et RDP effectives, SFTP, presse-papiers RDP, dossiers, modales, tunnels, snippets, accessibilité, navigation au clavier |

S'y ajoutent `clippy` en mode strict — **en profil debug et en profil release**,
qui ne voient pas le même code — ESLint typé, `cargo audit` sur les deux arbres
de dépendances, et une garde qui interdit les motifs dangereux (voir
[CONTRIBUTING.md](CONTRIBUTING.md)).

Une règle tient lieu de discipline : **un nouveau test doit avoir été vu
échouer**. On débranche ce qu'il couvre et on vérifie qu'il tombe — un test qui
ne peut pas échouer ne protège rien.

```bash
./check.sh              # tout valider
./check.sh --quick      # sans le build release
cd e2e && npm test      # tests bout en bout (ouvre des fenêtres)
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

## Licence

avash est distribué sous licence **[AGPL-3.0-or-later](LICENSE)**.

Vous pouvez l'utiliser, l'étudier, le modifier et le redistribuer librement. En
contrepartie, toute version modifiée — y compris **mise à disposition comme
service en réseau** — doit être publiée sous la même licence.

**Licence commerciale.** Pour intégrer avash dans un produit propriétaire ou
l'exploiter comme service sans publier vos modifications, une licence
commerciale distincte est disponible. Contact : adrien.cros@outlook.com

© 2026 Adrien Cros. Tous droits réservés.
