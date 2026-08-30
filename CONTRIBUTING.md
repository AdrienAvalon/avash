# Contribuer à Avash

Merci de l'intérêt que tu portes à Avash. Ce document décrit comment mettre en
place l'environnement, lancer les tests et proposer des changements.

## Prérequis

- **Rust stable** (édition 2021). Installe-le via [rustup](https://rustup.rs/),
  avec les composants `rustfmt` et `clippy`.
- **Node.js 22 ou supérieur** (le front et l'outillage E2E ciblent cette version).
- **Dépendances système Tauri** (Linux). Tauri s'appuie sur WebKitGTK et GTK :

  ```bash
  # Debian / Ubuntu
  sudo apt-get install -y \
    libwebkit2gtk-4.1-dev libgtk-3-dev \
    libayatana-appindicator3-dev librsvg2-dev patchelf
  ```

  Sur d'autres distributions, installe les paquets équivalents
  (`webkit2gtk-4.1`, `gtk3`, `libayatana-appindicator`, `librsvg`, `patchelf`).

## Installation

```bash
git clone <url-du-depot> avash
cd avash

# Dépendances du front
cd web && npm ci && cd ..
```

Le binaire release d'`avash-ui` embarque le front (`web/dist`) : le front doit
donc être construit avant le build Rust final. Le script de validation et la CI
s'en chargent dans le bon ordre.

## Lancer en développement

Le front se construit avec Vite, et l'application native avec Tauri :

```bash
# Front seul (rechargement à chaud), utile pour l'itération UI
cd web && npm run dev

# Application native complète (nécessite tauri-cli)
cargo install tauri-cli --version '^2.0' --locked   # une fois
cd crates/avash-ui && cargo tauri dev
```

Le **sidecar RDP** (`avash-rdp`) est un projet séparé, hors du workspace Cargo
(voir `docs/architecture.md`). Pour l'utiliser en développement, construis-le :

```bash
cd rdp-sidecar && cargo build --release
```

## Lancer les tests

La porte qualité complète est le script `check.sh` à la racine :

```bash
./check.sh            # tout : compilation, tests, format, clippy, audit, front, build release
./check.sh --quick    # idem, sans le build release final (boucle de dev plus rapide)
```

`check.sh` enchaîne, pour le cœur Rust et l'interface :

- `cargo check`, `cargo test`, `cargo fmt --check`, `cargo clippy -D warnings` ;
- `cargo audit` (si `cargo-audit` est installé) ;
- pour le front : la garde `scripts/guard.sh`, ESLint typé, `tsc --noEmit`,
  Vitest, puis le build Vite.

Les tests de bout en bout pilotent la **vraie application compilée** via
`tauri-driver` et WebdriverIO. Ils vivent dans `e2e/` :

```bash
# Recompiler l'app d'abord : le binaire release embarque le front
cargo build --release -p avash-ui -p test-rdp-server

cd e2e && npm install   # une fois
npm test                # toute la suite
```

Voir `e2e/README.md` pour les prérequis (`tauri-driver`, `webkit2gtk-driver`)
et le détail des 18 scénarios.

## Style de code

- **Commentaires en français.** Le code, les commentaires et les messages
  d'erreur destinés à l'utilisateur sont rédigés en français.
- **Rust : clippy en mode pédant.** Le socle est `clippy::pedantic` (déclaré
  dans `Cargo.toml` au niveau du workspace). Les avertissements sont traités
  comme des erreurs en CI (`clippy -D warnings`). Les rares exceptions sont
  déclarées et justifiées au niveau du workspace, pas ajoutées au cas par cas
  sans raison.
- **Front : ESLint typé + TypeScript strict.** Le front est vérifié par ESLint
  (configuration typée, `typescript-eslint`) et par `tsc --noEmit`.
- **Garde anti-étourderie (`scripts/guard.sh`).** Cette garde bloque avant
  commit les restes de mise au point (harnais de test, `debugger`,
  auto-connexion vers un serveur de test) **et** l'usage des dialogues natifs
  `confirm()` / `prompt()`. Ces dialogues sont **INOPÉRANTS sous WebKitGTK/WRY** :
  `confirm()` renvoie une `Promise` toujours vraie et `prompt()` renvoie
  toujours `null`. Utilise à la place les fonctions maison `askConfirm()` et
  `askText()`.

## Format des commits

Le dépôt suit la convention [Conventional Commits](https://www.conventionalcommits.org/),
avec le **message rédigé en français**. Types utilisés :

- `feat:` — nouvelle fonctionnalité ;
- `fix:` — correction de bug ;
- `perf:` — optimisation de performance ;
- `test:` — ajout ou modification de tests ;
- `chore:` — maintenance, outillage, dépendances ;
- `docs:` — documentation.

Exemples tirés de l'historique :

```
feat: arborescence de dossiers pour ranger les hôtes (SSH + RDP unifiés)
fix(front): confirmations de suppression contournées sous WebKitGTK + E2E étendu
perf: config release plus agressive (Rust) + bundle front allégé (vite)
```

## Licence des contributions

Avash est distribué sous licence **AGPL-3.0-or-later**. En proposant une
contribution, tu acceptes qu'elle soit intégrée et distribuée sous cette même
licence.
