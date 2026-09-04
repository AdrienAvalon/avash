# Image de base de la chaîne GitLab.
#
# Chaque job réinstallait par apt les mêmes paquets, Node, rustfmt, clippy et
# les outils cargo : une à deux minutes par job, cinq jobs par pipeline, et
# trois passages réels perdus sur des détails d'environnement (Node 18 de
# Debian, git 2.39 face à GitHub, /run/sshd). Tout est figé ici, une fois.
#
# L'image n'est jamais publiée : le job « image-ci » de .gitlab-ci.yml la
# construit sur le démon Docker du poste qui héberge l'exécuteur, et les autres
# jobs la prennent sur place (pull_policy: if-not-present). Sur un autre
# exécuteur, le premier pipeline la construit de même.
#
# La sortie d'apt est tue : GitLab est derrière le pare-feu applicatif de
# Cloudflare, qui rejette un journal contenant « /etc/ssh/sshd_config » (voir
# CONTRIBUTING.md, « Exécuteur GitLab »).
# Épinglée par empreinte : l'étiquette « 1-bookworm » bouge à chaque version
# de Rust, l'empreinte non. Dependabot (écosystème docker, répertoire ci/)
# propose la suivante ; le Scorecard de l'OpenSSF compte l'épinglage.
FROM rust:1-bookworm@sha256:82150a52ec202c1b14d7817e14516c392bb7f5cfebd88f1ed531cb37ebd39922

# Paquets système : Tauri (WebKitGTK, GTK, indicateur, rsvg, patchelf), le
# pilote WebDriver et Xvfb pour la suite bout en bout, sshd et openssl pour ses
# serveurs locaux, Opus et CMake pour le serveur RDP de test, Python et ses
# bibliothèques pour la conformité RDP.
RUN apt-get update -qq \
 && apt-get install -y -qq \
      libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf \
      webkit2gtk-driver xvfb socat openssh-server openssl libopus-dev cmake \
      python3 python3-numpy python3-pil python3-websockets \
      ca-certificates curl gnupg >/dev/null 2>&1 \
 && rm -rf /var/lib/apt/lists/* \
 && mkdir -p /run/sshd

# Node 22, comme la chaîne GitHub : bookworm n'en fournit que la 18, sous le
# plancher de stylelint et knip. Depuis l'archive officielle, vérifiée par sa
# somme publiée, et non par le script d'installation de NodeSource téléchargé
# puis exécuté : on n'exécute rien qu'on n'ait pu lire. Version et somme à
# reporter ensemble depuis https://nodejs.org/dist/latest-v22.x/SHASUMS256.txt.
ARG NODE_VERSION=22.23.2
ARG NODE_SHA256=d60acfe00a2932254bb0ad20e01b0d74397a0875595de719654b214f4b03f307
RUN curl -fsSLo /tmp/node.tar.xz "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-x64.tar.xz" \
 && echo "${NODE_SHA256}  /tmp/node.tar.xz" | sha256sum -c - >/dev/null \
 && tar -xJf /tmp/node.tar.xz -C /usr/local --strip-components=1 --exclude=CHANGELOG.md --exclude=LICENSE --exclude=README.md \
 && rm /tmp/node.tar.xz \
 && node --version && npm --version

# Chaîne Rust : les deux composants absents de l'image rust:1, puis les outils
# que les jobs installaient à chaque passage. Ils vivent dans
# /usr/local/cargo/bin, sur le PATH, indépendamment du CARGO_HOME que les jobs
# redirigent dans le projet pour mettre le registre en cache.
RUN rustup component add rustfmt clippy \
 && cargo install cargo-audit cargo-deny tauri-driver --locked \
 && rm -rf /usr/local/cargo/registry

# Client Docker statique, pour le job de conformité qui pilote le parc xrdp
# sur le démon de l'hôte (socket monté par l'exécuteur). Version épinglée et
# empreinte vérifiée : la même que le démon du poste au moment de l'écrire.
ARG DOCKER_VERSION=29.7.2
ARG DOCKER_SHA256=803d433f226db4776e1768fd319fc6c6e4935a456acf84fcc0080818b854bc8f
RUN curl -fsSL "https://download.docker.com/linux/static/stable/x86_64/docker-${DOCKER_VERSION}.tgz" -o /tmp/docker.tgz \
 && echo "${DOCKER_SHA256}  /tmp/docker.tgz" | sha256sum -c - >/dev/null \
 && tar -xzf /tmp/docker.tgz -C /usr/local/bin --strip-components=1 docker/docker \
 && rm /tmp/docker.tgz

# Le git 2.39 de bookworm reçoit un 401 de GitHub sur le POST git-upload-pack
# en HTTP/2 ; cargo-deny clone la base d'avis avec lui. HTTP/1.1 passe.
RUN git config --system http.version HTTP/1.1
