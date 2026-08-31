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
git clone https://github.com/AdrienAvalon/avash.git avash
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
(voir `docs/architecture.md`). Son binaire est déclaré en ressource embarquée
(`externalBin`) : **il doit exister avant toute compilation d'`avash-ui`**, sinon
le script de build de Tauri s'arrête sur `resource path ... doesn't exist`. Le
dossier `binaries/` n'est pas versionné — c'est un artefact. À faire une fois,
puis à refaire quand le sidecar change :

```bash
cargo build --release --manifest-path rdp-sidecar/Cargo.toml
mkdir -p crates/avash-ui/binaries
cp rdp-sidecar/target/release/avash-rdp \
   crates/avash-ui/binaries/avash-rdp-x86_64-unknown-linux-gnu
```

## Lancer les tests

La porte qualité complète est le script `check.sh` à la racine :

```bash
./check.sh            # tout : compilation, tests, format, clippy, audit, front, build release
./check.sh --quick    # idem, sans le build release final (boucle de dev plus rapide)
```

`check.sh` enchaîne :

- pour le workspace (cœur + interface) : `cargo check`, `cargo test`,
  `cargo fmt --check`, `cargo clippy -D warnings` — et **`clippy` une seconde
  fois en profil release**. Clippy ne compile qu'en debug : un bloc placé sous
  `cfg(debug_assertions)` peut laisser une variable orpheline en release sans
  que rien ne le signale. C'est arrivé ;
- pour le **processus RDP**, qui vit hors du workspace : ses propres `check`,
  `test`, `fmt` et `clippy`. Ils ne s'exécutaient nulle part pendant des
  semaines — `cargo test --workspace` ne le voit pas, et l'intégration continue
  se contentait de le compiler ;
- `cargo audit` sur **les deux** `Cargo.lock` (si `cargo-audit` est installé) ;
- pour le front : la garde `scripts/guard.sh`, ESLint typé, `tsc --noEmit`,
  Vitest, puis le build Vite ;
- au build release enfin : la construction du processus RDP **avant** celle
  d'`avash-ui`, qui en dépend par `externalBin`.

> **Toute crate ajoutée hors du workspace doit être branchée explicitement sur
> les trois portes** — `check.sh`, le hook de pré-commit et
> `.github/workflows/ci.yml`. Aucune ne la verra autrement.

Le hook `pre-commit` reprend l'essentiel : garde, format, clippy, tests Rust,
tests du processus RDP, et les trois vérifications rapides du front (`tsc`,
ESLint, Vitest). Il se contourne ponctuellement avec `git commit --no-verify`.

Les tests de bout en bout pilotent la **vraie application compilée** via
`tauri-driver` et WebdriverIO. Ils vivent dans `e2e/` :

```bash
# Recompiler l'app d'abord : le binaire release embarque le front
cargo build --release -p avash-ui -p test-rdp-server

cd e2e && npm install   # une fois
npm test                # toute la suite
```

Voir `e2e/README.md` pour les prérequis (`tauri-driver`, `webkit2gtk-driver`)
et le détail des 35 scénarios.

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
  `confirm()` / `prompt()` / `alert()`. Ces dialogues sont **INOPÉRANTS sous
  WebKitGTK/WRY** : `confirm()` renvoie une `Promise` toujours vraie,
  `prompt()` renvoie `null`, et `alert()` ne bloque pas. Utilise à la place les
  fonctions maison `askConfirm()`, `askText()` et `notify()`.

## Écrire un test

Une seule règle, mais elle n'est pas négociable : **un nouveau test doit avoir
été vu échouer**. Débranche ce qu'il couvre — inverse une condition, retire un
garde — et vérifie qu'il tombe, puis remets en état. Un test qui ne peut pas
échouer ne protège rien, et il coûte plus cher qu'il ne rapporte : il donne
l'illusion d'une couverture.

Deux corollaires appris à nos dépens :

- **Un serveur de test complaisant ne prouve rien.** Le nôtre rendait le même
  bloc quel que soit le décalage demandé : il aurait validé n'importe quel
  lecteur, y compris un lecteur parallèle qui réassemble de travers. Un serveur
  factice doit être aussi exigeant que la chose qu'il remplace.
- **Attendre un état, jamais une durée.** Les échecs intermittents viennent
  presque tous d'une interrogation faite trop tôt. Et « le port répond » n'est
  pas un état suffisant : un serveur qui traite ses clients l'un après l'autre
  doit être vu *revenir* accepter.

Un piège de manipulation, enfin : **ne défais jamais un contrôle négatif par
`git checkout <fichier>`** — il emporte tout le travail non commité du fichier.
Défais exactement l'édition que tu as faite.

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
