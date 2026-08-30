<div align="center">

<img src="web/public/hero.svg" alt="avash" width="200">

# avash

**Gestionnaire graphique de connexions SSH et RDP — natif, rapide, sécurisé.**

[![Licence: AGPL v3](https://img.shields.io/badge/licence-AGPL--3.0-blue.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-0.2.0-8b7cf6.svg)](CHANGELOG.md)
[![Tests](https://img.shields.io/badge/tests-223%20verts-brightgreen.svg)](#qualité)

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

**Sécurité**
- Mots de passe conservés uniquement dans le **trousseau du système** — jamais en clair sur le disque
- Mot de passe RDP transmis au processus RDP par entrée standard, jamais en argument de commande (invisible dans la liste des processus)
- Vérification des clés d'hôte SSH (TOFU) : connexion refusée si la clé change, avec une procédure explicite pour la réapprendre
- Aucune télémétrie, aucun appel réseau autre que vos connexions

## Installation

### Linux (AppImage)

```bash
chmod +x Avash_0.2.0_amd64.AppImage
./Avash_0.2.0_amd64.AppImage
```

### Compiler depuis les sources

Prérequis : Rust stable, Node.js 22+, et les dépendances système de Tauri.

```bash
# Debian / Ubuntu
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev \
    libayatana-appindicator3-dev librsvg2-dev patchelf

git clone <url-du-depot> avash && cd avash
(cd web && npm install && npx vite build)   # le binaire embarque le front
cargo build --release -p avash-ui
./target/release/avash-ui
```

Pour produire l'AppImage complète (avec le processus RDP embarqué) :

```bash
./scripts/release.sh
```

## Raccourcis

| Raccourci | Action |
|---|---|
| `Ctrl+K` | Palette de commandes |
| `Ctrl+W` | Fermer l'onglet |
| `Ctrl+Tab` | Onglet suivant |
| `Ctrl+1`…`9` | Aller à un onglet |
| `Ctrl+B` | Panneau de fichiers (SFTP) |

## Qualité

**223 tests** couvrent le projet, tous exécutés à chaque commit :

| Niveau | Nombre | Ce qui est vérifié |
|---|---|---|
| Rust (unitaires + intégration) | 139 | cœur SSH, SFTP, tunnels, config, secrets — dont des tests contre un vrai serveur SSH |
| Front (vitest) | 61 | logique pure : arborescence, filtres, encodage, entrées RDP |
| Bout en bout (WebdriverIO) | 23 | l'application réelle : connexion SSH et RDP effectives, SFTP, presse-papiers RDP, dossiers, modales, tunnels, snippets, accessibilité |

S'y ajoutent `clippy` en mode strict, ESLint typé, `cargo audit`, et une garde
qui interdit les motifs dangereux (voir [CONTRIBUTING.md](CONTRIBUTING.md)).

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
