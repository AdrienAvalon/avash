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

### Outillage de diagnostic (recommandé)

Rien de tout cela n'est nécessaire pour compiler ; tout l'est pour **chercher**
un défaut plutôt que le deviner. Chaque outil ci-dessous a servi au moins une
fois à trancher une question qu'aucun raisonnement n'aurait tranchée.

| Outil | Ce qu'il permet |
|---|---|
| `podman` ou `docker` | le parc RDP local (voir [tests-parc](../tests-parc/README.md)) |
| `python3-numpy`, `python3-pil` | le détecteur de cisaillement d'image |
| `freerdp` (`xfreerdp3`) | un client de référence, pour comparer notre rendu au sien |
| `tcpdump`, `tshark` | lire le flux RDP déchiffré (voir `scripts/tracer-rdp.sh`) |
| `strace` | voir où un processus se bloque vraiment |
| `perf` | profiler à l'échantillonnage, sans instrumenter |
| `hyperfine` | mesurer un temps d'exécution sans se raconter d'histoires |
| `cargo-deny` | licences, dépendances en joker, sources inconnues |
| `cargo-audit` | vulnérabilités déclarées |
| `cargo-nextest` | exécution des tests plus lisible et plus rapide |
| `python-websockets` | mesurer le trafic de trames RDP comme le fait l'interface |

```bash
# Arch / CachyOS
sudo pacman -S --needed podman freerdp tcpdump wireshark-cli \
  xorg-server-xvfb hyperfine python-numpy python-pillow python-websockets
cargo install cargo-audit cargo-deny cargo-nextest --locked
```

### Lire le flux RDP en clair

RDP est chiffré dès la négociation : `tcpdump` et `tshark` ne montrent que du
TLS. Mais la pile TLS du processus honore `SSLKEYLOGFILE` — capacité présente
depuis toujours, que personne n'avait employée. Avec les clés, `tshark` nomme
chaque PDU :

```bash
scripts/tracer-rdp.sh 127.0.0.1 3390 essai 'essai-mot-de-passe' 15 --sans-nla
```

```
  16    T.125    erectDomainRequest
  19    T.125    attachUserConfirm
  22    T.125    channelJoinConfirm 1003
  72    RDP      RDP PDU Type: Update
  92    RDP      Virtual Channel PDU 1004
```

C'est le complément du magnétoscope : celui-ci rejoue ce qu'on a compris, celui-là
montre ce qui passe réellement sur le fil, en-têtes compris. La chasse au défaut
de GNOME Remote Desktop aurait été bien plus courte avec.

**Le fichier de clés déchiffre TOUTE la session**, y compris l'échange CredSSP
qui porte le mot de passe. Le script l'écrit dans un répertoire temporaire privé
et l'efface en sortant ; ne le conservez pas, ne le joignez à aucun rapport.

### Traces du processus RDP

Le processus RDP sait raconter toute la séquence de connexion. C'est ce qui a
permis de voir qu'une connexion réputée bloquée sur NLA était en réalité
suspendue bien plus loin, dans la détection automatique du réseau.

```bash
AVASH_RDP_TRACE=ironrdp_connector=debug rdp-sidecar/target/release/avash-rdp \
  --host … -u … -p … --shot /tmp/ecran.png
```

**Ces traces contiennent le mot de passe en clair** — la requête CredSSP le
porte encodé en UTF-16, lisible tel quel. C'est pourquoi elles ne s'activent pas
sur `RUST_LOG`, que beaucoup exportent globalement, mais sur une variable qui
n'appartient qu'à nous. Relis avant de coller quoi que ce soit.

### Les captures d'écran du README

Elles sont prises sur l'application réelle, par le harnais de la suite bout
en bout, et se régénèrent après un build release (la pastille de version est
celle du binaire) :

```bash
scripts/captures-readme.sh                       # accueil et terminal SSH
scripts/captures-readme.sh 192.0.2.10            # plus un bureau Windows (mot de passe dans le trousseau)
```

Le bac à sable est semé d'hôtes plausibles (adresses documentaires), l'invite
du terminal est neutre : rien du poste ni du parc n'apparaît. `README.en.md`
partage les mêmes images.

### Un défaut d'affichage : enregistrer, rejouer, bissecter

Un bureau qui s'affiche mal se juge sur pièces, pas à l'œil. Le magnétoscope
enregistre tout ce que le serveur envoie, et le rejeu, sans réseau, dit si le
défaut vient de notre décodage :

```bash
# Depuis l'application : le processus RDP enregistre la session (0600).
AVASH_RDP_ENREGISTRER=/tmp/session.rec AVASH_RDP_ENREGISTRER_PLAFOND=536870912 avash-ui

# Rejouer, écrire l'image finale, s'arrêter après N PDU pour bissecter.
avash-rdp --rejouer /tmp/session.rec --image /tmp/fin.png
avash-rdp --rejouer /tmp/session.rec --jusqu-a 280 --image /tmp/pdu-280.png

# Décrire chaque commande du canal graphique : rectangles, tuiles en
# différence, tables de quantification, couches et sous-codecs ClearCodec.
AVASH_RDP_JOURNAL_EGFX=1 avash-rdp --rejouer /tmp/session.rec 2> /tmp/journal.txt
```

Puis **mesurer** la zone en cause (moyenne, écart-type, pixels sombres) avec
Python plutôt que la regarder : un « rectangle noir » lu sur une capture
réduite s'est révélé, à la mesure, être un gris uniforme. Un enregistrement
contient l'écran du serveur ; il reste sur le poste. Ceux qui reproduisent un
défaut corrigé vont dans `tests-parc/enregistrements/` avec leur empreinte et
un test qui dit ce qui doit être vrai. Pour un doute sur un codec, la
référence est FreeRDP (`libfreerdp/codec/`), pas IronRDP.

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
- pour les **correctifs portés sur IronRDP** (`rdp-sidecar/vendor`) : leurs
  tests propres, sans quoi un correctif pourrait être défait en silence lors
  d'une montée de version ;
- `cargo audit` sur **les deux** `Cargo.lock` (si `cargo-audit` est installé),
  et `cargo deny` sur les deux également : licences, dépendances en joker,
  sources hors du registre officiel — trois portes qu'`audit` ne regarde pas ;
- pour le front : la garde `scripts/guard.sh`, ESLint typé, stylelint sur le
  CSS d'`index.html`, knip (fichiers jamais importés, exports jamais lus,
  dépendances jamais utilisées — il a vu deux modules décrochés par un
  découpage, que ni `tsc` ni ESLint ne pouvaient voir), `tsc --noEmit`,
  Vitest, `npm audit` sur le front et sur la suite bout en bout, puis le
  build Vite ;
- au build release enfin : la construction du processus RDP **avant** celle
  d'`avash-ui`, qui en dépend par `externalBin`.

> **Toute crate ajoutée hors du workspace doit être branchée explicitement sur
> les quatre portes** — `check.sh`, le hook de pré-commit,
> `.github/workflows/ci.yml` et `.gitlab-ci.yml`. Aucune ne la verra autrement.

Le hook `pre-commit` reprend l'essentiel : garde, format, clippy, tests Rust,
tests du processus RDP, et les vérifications rapides du front (`tsc`, ESLint,
stylelint, knip, Vitest).

Trois regards extérieurs tournent sur GitHub, hors de `check.sh` : **CodeQL**
(Rust et TypeScript, constats dans l'onglet Security), **gitleaks** sur tout
l'historique, et le **Scorecard** de l'OpenSSF sur la posture du dépôt ;
**Dependabot** propose les mises à jour des quatre arbres de dépendances et des
actions. Deux outils servent à la main, de temps en temps : `cargo machete`
(dépendances déclarées mais jamais utilisées — cinq retirées le jour de sa
première exécution) et `cargo mutants` (force des tests : un mutant qui survit
est un test qui manque). Il est versionné dans `scripts/hooks/` ; un clone neuf
l'active une fois pour toutes :

```bash
git config core.hooksPath scripts/hooks
```

Il se contourne ponctuellement avec `git commit --no-verify`.

Les tests de bout en bout pilotent la **vraie application compilée** via
`tauri-driver` et WebdriverIO. Ils vivent dans `e2e/` :

```bash
# Recompiler l'app d'abord : le binaire release embarque le front
cargo build --release -p avash-ui
# Les serveurs de test (RDP, VNC), hors espace de travail
cargo build --release --manifest-path test-rdp-server/Cargo.toml
cargo build --release --manifest-path test-vnc-server/Cargo.toml

cd e2e && npm install   # une fois
npm test                # toute la suite
```

**Régression visuelle.** `specs/visuel.spec.js` compare des captures de
l'interface (accueil sur les deux thèmes, palette, modale) à des références,
pixel à pixel, avec une tolérance d'un demi pour-cent. Les références de
`e2e/visuel/reference` sont celles de la chaîne (ubuntu-latest) : les polices
d'une autre machine ne rendent pas pareil, donc en local le scénario est sauté
sauf `VISUEL=1 npx wdio run wdio.conf.js --spec specs/visuel.spec.js` — un
passage à part, car le service de comparaison ralentit chaque fichier de
scénarios —, et ses captures vont dans `e2e/.tmp`, ignoré. Après
un changement d'interface voulu, rafraîchir les références : récupérer
l'artefact « visuel » du job E2E, copier son dossier `reference` dans
`e2e/visuel/reference`, commiter.

**Sous Windows.** Pas de pilote natif : Edge WebDriver ne lance plus une
application WebView2 depuis sa version 133 (« DevToolsActivePort file doesn't
exist »). L'application est compilée avec son serveur WebDriver embarqué, et
c'est le harnais qui la lance à chaque fichier de scénarios :

```powershell
cargo build --release -p avash-ui --features webdriver
cd e2e; npm test
```

La chaîne joue la suite complète à chaque poussée (job `e2e-windows`),
serveurs locaux compris : le sshd est le service OpenSSH Server de
l'exécuteur, les serveurs RDP et VNC de test sont construits sur place. Seuls
l'import PuTTY, la régression visuelle, les vraies touches et la réouverture
des onglets se sautent sur ce chemin. Le même chemin se force partout avec
`E2E_EMBARQUE=1` — c'est celui de macOS en chaîne (scénarios sans serveur,
`E2E_NO_RDP=1`), et le moyen de vérifier le harnais sous Linux avant de
pousser : le serveur embarqué ne transmet pas tout au DOM (double-clic,
caractères tapés), et `e2e/README.md` dit comment les scénarios s'en
accommodent. La fonctionnalité `webdriver` n'entre jamais dans un binaire
publié.

Voir `e2e/README.md` pour les prérequis (`tauri-driver`, `webkit2gtk-driver`)
et le détail des 69 scénarios.

### Claude sur les issues et les PR

Une mention `@claude` dans une issue, un commentaire ou une revue de PR
déclenche le workflow `claude.yml` : Claude lit le dépôt et la chaîne, répond,
propose un correctif ou résume. Chaque PR ouverte reçoit en outre une revue
automatique en commentaires en ligne (`claude-code-review.yml`), sauf celles
de Dependabot. Les deux tournent sur le jeton de l'abonnement Claude Code du
mainteneur (secret `CLAUDE_CODE_OAUTH_TOKEN`) ; une PR venue d'un fork n'y a
pas accès, et c'est voulu.

### Fuzzing (nightly, optionnel)

`fuzz/` secoue les parseurs du cœur avec cargo-fuzz ; il exige nightly, donc
il est hors de l'espace de travail et de `check.sh`. Une fois
`rustup toolchain install nightly` et `cargo install cargo-fuzz --locked`
passés : `fuzz/fuzz.sh` (60 s par cible), ou plus longtemps avec `DUREE=600`.
Une entrée qui fait paniquer atterrit dans `fuzz/artifacts/<cible>/` ; elle
se rejoue avec `cargo +nightly fuzz run <cible> <fichier>` et mérite un test
unitaire à côté du correctif. Toucher à un parseur (`parse_config_str`,
`import.rs`, `enregistrement::relire`) = relancer la cible correspondante.
Détails et invariants dans `fuzz/README.md`.

### Conformité RDP : le niveau qui manquait

Les tests unitaires vérifient nos fonctions. La suite bout en bout vérifie
l'interface. Entre les deux vivait un angle mort : le **dialogue réel avec un
serveur RDP**. Les trois défauts de la 0.3.3 y logeaient tous, et tous ont été
signalés par l'usage plutôt que par la machine.

```bash
scripts/parc-rdp.sh up tous        # xrdp XFCE (3390), xrdp GNOME (3391), sshd (2222)
scripts/conformite.sh tous
scripts/parc-rdp.sh down

# ou intégré à la porte complète
scripts/parc-rdp.sh up tous && CONFORMITE_RDP=1 PARC=tous ./check.sh
```

Six contrôles, un par défaut rencontré : la connexion RDP aboutit, l'image n'est
pas cisaillée, les trames ne renvoient pas un plein écran pour deux poussières,
la disposition clavier annoncée n'est pas zéro, le repli SSH
`keyboard-interactive` fonctionne contre un serveur qui refuse `password`, et
SFTP fait l'aller-retour à l'octet près.

Ce que le parc **ne** couvre pas — GNOME Remote Desktop, Windows, la lecture du
flux RDP au fil — est dit sans détour dans
Voir [tests-parc/README.md](tests-parc/README.md).

## Exécuteur GitLab

Le dépôt est poussé sur GitHub **et** sur GitLab. Pendant longtemps seul GitHub
vérifiait quoi que ce soit : GitLab recevait chaque poussée sans rien contrôler,
et le miroir avait pris cinquante commits de retard. La chaîne équivalente de
`.gitlab-ci.yml` tourne depuis le 3 septembre 2026 sur un exécuteur enregistré
sur le poste du mainteneur (`avalon-cachyos`, runner 18 du projet) : Docker
privilégié, socket du démon monté dans les travaux, trois travaux en parallèle.
Sa configuration vit dans `/etc/gitlab-runner/config.toml` (jeton
d'enregistrement compris, lisible par personne d'autre), le service est
`gitlab-runner.service`. Le poste éteint, les travaux attendent ; ils
reprennent au démarrage suivant.

Les travaux tournent dans une image de base, `avash-ci:bookworm`, décrite par
`ci/Dockerfile` : paquets système, Node 22, rustfmt, clippy, les outils cargo et
le client Docker y sont figés, au lieu d'être réinstallés par chaque travail à
chaque passage. Elle n'est publiée nulle part (l'instance n'a pas de registre
de conteneurs, et le pare-feu devant elle limite la taille des envois) : le
premier travail de chaque pipeline, `image-ci`, la construit sur le démon
Docker de l'hôte — quelques secondes quand rien n'a changé — et les autres la
prennent sur place (`pull_policy: if-not-present`). Le travail de conformité
pilote le parc xrdp sur ce même démon : les trois images du parc sont
construites une fois et gardées, et leurs conteneurs vivent sur un réseau
Docker dédié, sans publier de port, auquel le travail se raccorde ; rien
n'écoute sur le réseau du poste.

Ce montage donne à chaque travail la main sur le démon Docker de l'hôte, ce
qui vaut la racine : c'est déjà le cas d'un exécuteur privilégié, et le code
qui y tourne est celui du dépôt. Ne pas y enregistrer un exécuteur partagé.

Pour en déclarer un autre :

```bash
# Sur la machine qui hébergera l'exécuteur
sudo pacman -S docker gitlab-runner # ou les paquets de ta distribution
sudo systemctl enable --now docker
sudo usermod -aG docker gitlab-runner
sudo gitlab-runner register \
  --url https://gitlab.avalon-network.com \
  --executor docker \
  --docker-image rust:1-bookworm \
  --docker-privileged \
  --docker-volumes /cache \
  --docker-volumes /var/run/docker.sock:/var/run/docker.sock
# Puis, dans /etc/gitlab-runner/config.toml : concurrent = 3 en tête, et
# allowed_pull_policies = ["always", "if-not-present"] sous [runners.docker].
sudo systemctl enable --now gitlab-runner
```

Le premier pipeline construit l'image de base sur ce nouvel hôte ; rien
d'autre à préparer.

Le jeton d'inscription se prend dans **Paramètres → CI/CD → Runners** du projet
(« Nouveau runner de projet »), ou par l'API avec un jeton personnel portant
`create_runner` : `POST /api/v4/user/runners` avec `runner_type=project_type`
et `project_id`. Il ne se colle dans aucun fichier suivi : `register` l'écrit
lui-même dans `config.toml`.

**Un travail qui meurt sans message.** GitLab est derrière le pare-feu
applicatif de Cloudflare, qui inspecte chaque envoi de journal du runner. Une
ligne d'allure suspecte à ses yeux (un chemin système comme
`/etc/ssh/sshd_config`, `/etc/passwd`…) lui fait rejeter l'envoi en 403 ; le
runner annule alors le travail, que GitLab marque échoué avec la raison
`unknown_failure`, et la trace s'arrête net juste avant la ligne fautive. Pour
voir ce qui a réellement été imprimé, brancher `sudo docker logs -f` sur le
conteneur `runner-…-build` pendant que le travail tourne ; pour identifier la
ligne, envoyer les morceaux du journal au pare-feu (`curl -X PATCH
--data-binary @morceau https://gitlab.avalon-network.com/api/v4/jobs/1/trace`
répond en JSON quand le morceau passe, en HTML quand Cloudflare le bloque).
Le remède est de faire taire la commande fautive (voir l'installation
d'`openssh-server` dans `.gitlab-ci.yml`) ou, mieux, d'exempter
`/api/v4/jobs/*/trace` des règles du pare-feu.

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
