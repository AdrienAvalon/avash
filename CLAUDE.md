# Avash — consignes pour Claude

Gestionnaire de connexions SSH et RDP, natif, en Rust (Tauri 2, russh,
IronRDP) avec un front TypeScript (xterm.js). Ce fichier est lu par Claude
Code en session locale et par les workflows GitHub `@claude` et de revue :
ce qui n'est pas ici n'existe pas pour eux.

## Langue et ton

- Tout en **français** : réponses, commentaires de code, messages de commit,
  documentation, noms de tests (`fn le_port_zero_n_est_pas_un_port`).
  Identifiants techniques et termes consacrés restent tels quels.
- Orthographe complète, accents compris. Pas de tirets cadratins dans la prose.
- Tutoiement avec le mainteneur.

## Disposition du dépôt

| Chemin | Rôle |
|---|---|
| `crates/avash` | cœur : config SSH, clés, secrets, import PuTTY/MobaXterm, enregistrement asciicast, santé des hôtes, tunnels, snippets |
| `crates/avash-ui` | interface Tauri ; commandes dans `src/commands/` (un fichier par domaine), `src/rdp.rs`, `src/langue.rs` |
| `web/` | front TypeScript (Vite, Vitest, ESLint, stylelint, knip) ; i18n dans `web/i18n.ts` |
| `rdp-sidecar/` | processus de bureau distant (RDP par IronRDP, VNC par vnc-rs dans `src/vnc.rs`), **hors espace de travail**, avec des paquets IronRDP et vnc-rs portés dans `rdp-sidecar/vendor/` |
| `test-rdp-server/` | serveur RDP de test pour la suite bout en bout (presse-papiers, son, lecteur RDPDR côté serveur), hors espace de travail, avec un ironrdp-server porté dans `test-rdp-server/vendor/` |
| `test-vnc-server/` | serveur VNC de test (rustvncserver), hors espace de travail, qui réagit aux entrées pour que le scénario mesure les pixels |
| `e2e/` | suite WebdriverIO sur la vraie application (Linux : tauri-driver + WebKitWebDriver ; Windows et macOS : serveur WebDriver embarqué) |
| `fuzz/` | cibles cargo-fuzz (nightly), hors espace de travail |
| `ci/` | `Dockerfile` de l'image de base de la chaîne GitLab, construite sur le démon Docker du runner par le premier job de chaque pipeline |
| `docs/` | `architecture.md`, `feuille-de-route.md` ; `CHANGELOG.md`, `SECURITY.md`, `CONTRIBUTING.md`, `RELEASE.md` à la racine |
| `packaging/` | canaux de distribution : `aur/` (PKGBUILD), `flathub/` (manifeste et sources figées, régénérées par `scripts/flathub-sources.sh`), `homebrew/`, `winget/`, métadonnées AppStream |
| `site/` | vitrine GitHub Pages (FR à la racine, EN dans `en/`), publiée par `pages.yml` ; les captures viennent de `docs/captures` |
| `secrets/` | fichiers chiffrés sops (jeton GitHub, aide git-credential) ; **jamais en clair, jamais commités déchiffrés** |

## Valider avant de livrer

- `./check.sh` est la porte : format, clippy pédant (`-D warnings`, debug et
  release), tests Rust du cœur, de l'interface et du sidecar, audit et deny,
  front (tsc, eslint, stylelint, knip, vitest, npm audit). Il tourne aussi dans
  le hook pre-commit (`scripts/hooks`, activé par `core.hooksPath`).
- `./check.sh --quick` saute le build release : **toujours reconstruire en
  release avant la suite bout en bout**, le binaire embarque `web/dist`.
- Le hook pre-commit refuse un commit non formaté et le fait **sans bruit**
  (le journal reste sur l'ancien HEAD) : `cargo fmt -p <crate>` avant de
  commiter, puis vérifier `git log -1`.
- Clippy ne compile qu'en debug : un bloc `cfg(debug_assertions)` peut cacher
  un avertissement release. `check.sh` passe les deux.
- Suite bout en bout : `cd e2e && xvfb-run -a npm test` (serveurs locaux
  compris). Sous-ensemble sans serveur : `E2E_NO_RDP=1`. Chemin embarqué
  (Windows, macOS, ou `E2E_EMBARQUE=1` partout) : binaire compilé avec
  `cargo build --release -p avash-ui --features webdriver`.
- Toute commande longue se lance par PID, jamais `pkill -f` avec un motif
  contenu dans sa propre ligne de commande (le shell s'est déjà tué ainsi).
- Un test qui a échoué se corrige à la main : jamais `git checkout <fichier>`
  pour annuler, il emporte tout le travail non commité.

## Tests : ce qu'on attend

- Chaque correctif vient avec le test qui l'aurait vu. Les tests nomment le
  comportement en français, et le commentaire dit d'où vient le cas
  (« trouvé par cargo-fuzz », « régression vue en CI »).
- Un nouveau parseur reçoit une cible dans `fuzz/` et une graine dans
  `fuzz/seeds/`. Quatre défauts ont été trouvés en moins d'une minute chacun.
- Le sidecar RDP est hors espace de travail : `cargo test --workspace` ne le
  voit pas, `check.sh` et la CI le lancent séparément.
- Les compteurs de tests figurent dans `README.md` et `README.en.md` (badge et
  section Qualité), `docs/qualite.md`, `docs/feuille-de-route.md` et le site
  (`site/index.html`, `site/en/index.html`) : les mettre à jour quand ils
  changent. Les badges « couverture » et « mutants » des README sont
  statiques eux aussi : ils suivent le relevé du workflow qualité consigné
  dans `docs/qualite.md`.

## Sécurité et secrets

- Jamais de secret dans un fichier suivi, un message de commit ou un
  commentaire. Les jetons vivent dans `secrets/github.enc.yaml` et
  `secrets/gitlab.enc.yaml` (sops, clé age du poste) et servent via
  `git push github` et `git push gitlab`, par l'aide d'identifiants
  `secrets/git-credential-sops.sh` ; `gh` est connecté sur cette machine.
  Le dépôt est poussé sur les **deux** dépôts distants.
- Les durcissements (WebKit inspector, `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`,
  TOFU SSH et RDP, écritures atomiques 0600) sont documentés dans
  `SECURITY.md` et `docs/architecture.md` : ne pas les affaiblir sans y écrire
  pourquoi.
- La fonctionnalité cargo `webdriver` (serveur WebDriver embarqué) ne doit
  **jamais** entrer dans un binaire publié : seuls les jobs E2E la posent.

## Chaîne d'intégration et publication

- Actions GitHub **épinglées sur leur commit** avec la version en commentaire
  (`uses: owner/action@<sha> # vX.Y.Z`) ; `permissions` minimales ; groupe de
  concurrence sur `ci.yml` et `securite.yml`.
- Dependabot ouvre les montées de version. Les majeures se traitent en lot et
  à la main quand elles vont ensemble (les trois étapes de codeql-action) ;
  les refus sont notés dans `.github/dependabot.yml` avec leur raison.
- Windows : Edge WebDriver ≥ 133 ne lance plus une application WebView2 ; la
  suite passe par le serveur embarqué. macOS : même chemin, aucun pilote.
- Publication : voir `RELEASE.md` (version dans `Cargo.toml` de l'espace de
  travail, `CHANGELOG.md` section `[Non publié]` → version datée, tag `vX.Y.Z`,
  workflow Release, `latest.json` signé). Le mainteneur décide du moment.
- Vérifier une chaîne : API GitHub avec le jeton sops via `/usr/bin/curl`, ou
  `gh run list` ; ne jamais conclure sur un job encore en cours.

## Documentation à tenir à jour

À chaque changement visible : `CHANGELOG.md` (`[Non publié]`), puis selon le
cas `README.md` **et sa traduction `README.en.md`** (même structure, mêmes
chiffres), `docs/qualite.md`, `docs/feuille-de-route.md` (axes et compteurs),
`docs/architecture.md`, `CONTRIBUTING.md`, `e2e/README.md`, `SECURITY.md`.
Les captures du README se régénèrent par `scripts/captures-readme.sh` après
un build release (la version affichée est celle du binaire).
Les commentaires de code expliquent le **pourquoi** et l'histoire du cas
(le bug vu, la mesure faite), pas le quoi.

## Format des commits

`type(portée): résumé en français` (fix, feat, test, ci, docs, build,
refactor), corps qui raconte la cause et la décision. Signature
`Co-Authored-By` de Claude quand Claude écrit. Détails dans `CONTRIBUTING.md`.
