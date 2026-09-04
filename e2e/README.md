# Tests E2E — Avash

Pilotent la **vraie application compilée** (WebKitGTK) via `tauri-driver` + WebdriverIO.
C'est le seul niveau qui attrape les bugs du runtime réel : par exemple, sous
WebKitGTK/WRY, `window.confirm()` ne bloque pas et renvoie une Promise toujours vraie,
et `window.prompt()` renvoie toujours `null` — deux pièges invisibles en unitaire.

## Prérequis (une fois)

```bash
cargo install tauri-driver --locked
# WebKitWebDriver : fourni par webkitgtk-6.0 (Arch/CachyOS : « paru -S webkitgtk-6.0 »)
# Le driver 6.0 pilote l'app même liée à webkit2gtk-4.1.
cd e2e && npm install
```

`tauri-driver` est lancé une fois pour toute la suite et enchaîne le pilote
natif. Avant chaque fichier, la configuration vérifie qu'il répond encore
(`/status`) et le relance sinon, sur un port natif neuf : le natif est mort une
fois en pleine suite sur la chaîne GitLab (#3382), emportant tout ce qui
suivait.

Le même harnais prend les **captures d'écran du README** (`docs/captures/`) :
`scripts/captures-readme.sh [hôte RDP]` lance `wdio.captures.conf.js` sous
Xvfb, avec le bac à sable semé d'hôtes plausibles, une session SSH ouverte sur
le sshd local et, si un hôte est donné, un bureau Windows (mot de passe lu dans
le trousseau, jamais en argument). Le même parcours dépose des cadres aux
moments clés, que le script monte en démonstration animée (`demo.webp`,
ffmpeg). Ce n'est pas un test : rien n'est comparé, et `captures/` n'est pas
dans la liste des spécifications.

## Lancer

```bash
# Depuis la racine : construire l'app (le binaire EMBARQUE le front) et les serveurs de test
cargo build --release -p avash-ui
cargo build --release --manifest-path test-rdp-server/Cargo.toml
cargo build --release --manifest-path test-vnc-server/Cargo.toml
cd e2e && npm test                    # toute la suite
npx wdio run wdio.conf.js --spec specs/rdp.spec.js   # un seul fichier
```

> Après toute modification du front, **recompiler `avash-ui`** : le binaire release
> embarque `web/dist`. Un simple `vite build` ne suffit pas pour l'E2E.

## Isolation

`wdio.conf.js` (`onPrepare`) crée un `HOME`/`XDG_CONFIG_HOME` temporaire et y **sème**
une config SSH de test (hôtes `web-1` rangé dans `prod`, `db-1` à la racine) — aucun
effet sur la vraie config. Il démarre aussi un **serveur RDP de test** local
(`127.0.0.1:33899`, identifiants `test`/`test`) pour `rdp.spec.js`, et
`vnc.spec.js` lance le **serveur VNC de test** (`test-vnc-server/`, port 35900,
mot de passe `test`), qui sert une image connue et réagit aux entrées.

## Couverture (63 scénarios, 32 fichiers)

| Fichier | Ce qui est vérifié |
|---|---|
| `smoke.spec.js`       | démarrage, barre latérale, accueil |
| `hosts.spec.js`       | rendu des hôtes semés, dossier `prod`, sélection (`.picked`) |
| `hosts-move.spec.js`  | déplacer un hôte dans un dossier via « Déplacer vers… » |
| `folders.spec.js`     | cycle de vie complet : créer, sous-dossier, renommer, **supprimer** (modale maison), **annulation respectée** |
| `snippets.spec.js`    | snippet : créer, lister, **supprimer** (askConfirm) |
| `tunnels.spec.js`     | tunnel local : créer, lister, **supprimer** (askConfirm) |
| `a11y.spec.js`        | **accessibilité** : role=dialog + titre accessible, piège de focus (Tab ne fuit pas), focus rendu au déclencheur, noms accessibles des boutons icône |
| `axe.spec.js`         | **audit axe-core** de l'application réelle : vue principale, thème clair, boîte de connexion manuelle (voir plus bas) |
| `isolation.spec.js`   | **garde-fou d'isolation** : chaque fichier part de l'état semé, sans reste des autres scénarios |
| `modals.spec.js`      | « Connexion directe » ne se ferme pas au clic dehors, se ferme à Échap ; palette Ctrl+K |
| `ssh.spec.js`         | **connexion SSH réelle** (sshd local, auth par clé) → session live |
| `sftp.spec.js`        | **panneau SFTP** sur la session SSH : listing du répertoire distant, **téléchargement d'un dossier entier** par la file des transferts (octets comparés), **copie d'un fichier vers un autre onglet SSH** sans passer par le disque du poste |
| `rdp.spec.js`         | **connexion RDP réelle** (serveur dédié) → handshake CredSSP + canvas (`.state.live`) |
| `rdp-clipboard.spec.js` | **presse-papiers RDP** (distant → poste) : pilote le sidecar sur son WebSocket, sans toucher au presse-papiers du système |
| `rdp-reconnect.spec.js` | **overlay de reconnexion** quand le serveur RDP coupe |
| `rdp-fichiers.spec.js` | **fichiers par le presse-papiers RDP**, dans les deux sens : liste annoncée sans contenu, réception après accord (2,5 Mo, octets comparés), offre d'un fichier du poste reçu par le serveur (300 Ko) |
| `vnc.spec.js`         | **connexion VNC réelle** (serveur dédié, ZRLE) : pixels rouge et bleu, carré magenta au clic, bureau vert après « g », keysym 0xe9 pour « é », mauvais mot de passe refusé avec sa raison |
| `clavier.spec.js`     | palette aux flèches, `Ctrl+K` bloqué par-dessus une boîte, Échap ne ferme qu'une boîte à la fois |
| `liste-clavier.spec.js` | **barre latérale au clavier** : un seul arrêt de tabulation, flèches et Origine/Fin, focus qui vaut sélection, `Maj+F10` et navigation dans le menu |
| `prefs.spec.js`       | réglage du **partage de presse-papiers** : présent à la palette, bascule, retenu, libellé qui annonce l'état courant |
| `resize.spec.js`      | l'application reste répondante après une rafale de redimensionnements |
| `onglets-mixtes.spec.js` | SSH et RDP côte à côte : bascule d'onglets, fermeture, l'autre survit |
| `enregistrer-et-connecter.spec.js` | « Enregistrer et connecter » depuis la modale de connexion directe |
| `enregistrement.spec.js` | **enregistrement asciicast** sur la session SSH réelle : l'écran initial, la sortie, le fichier ; la liste dans la palette |
| `sante.spec.js`       | **santé des hôtes** : voyant vert sur le sshd local, rouge sur une adresse sans route, résultat retenu |
| `import.spec.js`      | **import PuTTY** : sessions semées dans `.putty/sessions`, aperçu, application (hors Windows) |
| `langue.spec.js`      | **langue** imposée par `AVASH_LANGUE`, bascule à la palette, retenue |
| `diagnostic.spec.js`  | **export d'un diagnostic** : l'entrée de palette, la commande écrit un fichier lisible, sans alias ni adresse de la configuration |
| `restauration.spec.js` | **mémoire des onglets** : session SSH ouverte, front rechargé, proposition de rouvrir, réouverture live ; « Ignorer » efface |
| `vue-partagee.spec.js` | **vue partagée** : `Ctrl+Maj+E` met deux sessions SSH côte à côte (deux volets, largeurs voisines), fermer l'une referme le partage |
| `serie.spec.js`       | **port série** sur un pseudo-terminal `socat` qui renvoie ce qu'il reçoit : connexion directe en mode Série, session live, écho reçu par `pty-output` (Linux) |
| `visuel.spec.js`      | **régression visuelle** : accueil sur les deux thèmes, palette, modale, contre `visuel/reference` (`VISUEL=1`, Linux) |

Chaque fichier de tests repart de l'état semé (`beforeSession` remet le bac à sable à zéro).
Serveurs locaux : chaque spec RDP démarre son propre serveur de test (aucun couplage) ;
un **sshd non-root** (port 2223, clé) est monté dans `onPrepare` pour `ssh.spec`.

En CI (`E2E_NO_RDP=1`), la configuration **exclut** les fichiers qui exigent
un serveur local (`ssh`, `sftp`, `rdp`, `rdp-reconnect`, `rdp-clipboard`, `rdp-fichiers`, `vnc`,
`onglets-mixtes`, `enregistrer-et-connecter`, `enregistrement`, `sante`,
`restauration`, `vue-partagee`, `serie`).
C'est une exclusion et non une énumération de ce qui tourne :
la liste énumérative prenait du retard à chaque scénario ajouté, et cinq
scénarios pourtant sans serveur ne tournaient plus qu'en local. Une nouvelle
spec sans serveur tourne désormais en intégration continue d'office.

L'attendu d'`isolation.spec.js` est **dérivé du semage** (`HOTES_SEMES`, exporté
par `wdio.conf.js`) : le réénoncer l'avait rendu faux en CI, où l'hôte
réellement joignable n'est pas semé.

## Sous Windows : le serveur WebDriver embarqué

Même suite, autre chemin vers l'application. Edge WebDriver — le pilote natif
que `tauri-driver` enchaîne sous Windows — ne lance plus une application
WebView2 depuis sa version 133 : chaque session mourait après quatre minutes
sur « DevToolsActivePort file doesn't exist », l'exception d'automatisation du
durcissement `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` n'y changeait rien.
L'application est donc compilée avec `--features webdriver` : elle embarque un
serveur WebDriver (`tauri-plugin-wdio-webdriver`, 127.0.0.1:4445) et le harnais
la lance lui-même dans `beforeSession`, attend `/status`, puis WebdriverIO lui
parle en direct ; `afterSession` l'arrête. Une application par fichier de
scénarios, comme sous Linux. Les serveurs locaux ne sont pas montés
(`LOCAL_SERVERS` est faux), l'import PuTTY et la régression visuelle sont
sautés. Le job `e2e-windows` de la chaîne joue ce sous-ensemble.

`E2E_EMBARQUE=1` force ce chemin sur toute plateforme (Linux compris, sur un
binaire compilé avec la fonctionnalité) : c'est ainsi que le harnais se
vérifie avant de pousser. C'est aussi le chemin de macOS (WKWebView), qui n'a
aucun pilote : le job macOS de la chaîne joue le même sous-ensemble.

Une limite connue : le serveur embarqué synthétise les touches en JavaScript
(Origine et Fin non traduites, flèches gérées seulement sur des boutons radio,
modificateurs ignorés sur les touches de fonction). `liste-clavier.spec.js`,
qui a besoin de vraies touches, se saute sur ce chemin et reste joué sous
Linux.

## Astuces WebKitGTK

- `getText()` renvoie parfois vide → lire `getProperty("textContent")`.
- Le clic droit ne génère pas d'`contextmenu` → le dispatcher (`helpers.openCtx`).
- Les radios stylées ne sont pas « interactables » → cocher via `browser.execute` + event `change`.
- **Attendre un état, jamais une durée.** Les `browser.pause` ont tous disparu :
  ils mesuraient la charge de la machine plus que le comportement. Pour un cas
  « rien ne doit changer », attendre un événement observable — un aller simple
  jusqu'au moteur via `requestAnimationFrame` — puis constater.
- `waitForPort` exige **deux** connexions successives : une seule prouve que le
  socket écoute, pas que le serveur est *revenu* l'écouter. Il traite ses
  clients l'un après l'autre, et notre sonde en est un.

## Dépendances : les `overrides` de `package.json`

WebdriverIO 9 tire `deepmerge-ts` 7 (épuisement de pile sur un graphe
récursif) et, par mocha, `serialize-javascript` 6 (exécution de code par
`RegExp.flags`), et aucune de ses versions ne les corrige : `npm audit fix` ne
propose qu'une rétrogradation en 7.x. Les deux `overrides` imposent les
versions corrigées ; la suite tourne pareil. Les avis restants viennent tous
d'`extract-zip`, sans correctif amont : c'est du code de test, jamais
embarqué, et `npm audit` n'y bloque que sur « critique » pour cette raison.
Si une montée de WebdriverIO corrige l'un des deux, retirer l'`override`
correspondant.

## Audit d'accessibilité (`axe.spec.js`)

`axe-core` est injecté dans l'application réelle et audite la vue principale, le
thème clair et la boîte de connexion manuelle.

Deux précautions, apprises en écrivant ce test :

- **Geler transitions et animations AVANT toute bascule de thème.** Sans cela,
  l'audit lit des couleurs à mi-transition — exactement à mi-chemin entre les
  deux palettes — et invente des violations qui n'existent dans aucune image
  réellement affichée.
- **Basculer le thème par le bouton**, comme l'utilisateur, et non en posant
  `data-theme` à la main : l'application repilote cet attribut depuis sa
  préférence, et l'écrasait en cours d'audit.

Les deux fois, c'est l'instrument qui mentait, pas le style. Un test
d'accessibilité qui produit de faux positifs finit ignoré, donc inutile.
