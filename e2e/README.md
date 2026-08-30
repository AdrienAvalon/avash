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

## Lancer

```bash
# Depuis la racine : construire l'app (le binaire EMBARQUE le front) + le serveur RDP de test
cargo build --release -p avash-ui -p test-rdp-server
cd e2e && npm test                    # toute la suite
npx wdio run wdio.conf.js --spec specs/rdp.spec.js   # un seul fichier
```

> Après toute modification du front, **recompiler `avash-ui`** : le binaire release
> embarque `web/dist`. Un simple `vite build` ne suffit pas pour l'E2E.

## Isolation

`wdio.conf.js` (`onPrepare`) crée un `HOME`/`XDG_CONFIG_HOME` temporaire et y **sème**
une config SSH de test (hôtes `web-1` rangé dans `prod`, `db-1` à la racine) — aucun
effet sur la vraie config. Il démarre aussi un **serveur RDP de test** local
(`127.0.0.1:33899`, identifiants `test`/`test`) pour `rdp.spec.js`.

## Couverture (22 scénarios)

| Fichier | Ce qui est vérifié |
|---|---|
| `smoke.spec.js`       | démarrage, barre latérale, accueil |
| `hosts.spec.js`       | rendu des hôtes semés, dossier `prod`, sélection (`.picked`) |
| `hosts-move.spec.js`  | déplacer un hôte dans un dossier via « Déplacer vers… » |
| `folders.spec.js`     | cycle de vie complet : créer, sous-dossier, renommer, **supprimer** (modale maison), **annulation respectée** |
| `snippets.spec.js`    | snippet : créer, lister, **supprimer** (askConfirm) |
| `tunnels.spec.js`     | tunnel local : créer, lister, **supprimer** (askConfirm) |
| `a11y.spec.js`        | **accessibilité** : role=dialog + titre accessible, piège de focus (Tab ne fuit pas), focus rendu au déclencheur, noms accessibles des boutons icône |
| `modals.spec.js`      | « Connexion directe » ne se ferme pas au clic dehors, se ferme à Échap ; palette Ctrl+K |
| `ssh.spec.js`         | **connexion SSH réelle** (sshd local, auth par clé) → session live |
| `sftp.spec.js`        | **panneau SFTP** sur la session SSH → listing du répertoire distant |
| `rdp.spec.js`         | **connexion RDP réelle** (serveur dédié) → handshake CredSSP + canvas (`.state.live`) |
| `rdp-reconnect.spec.js` | **overlay de reconnexion** quand le serveur RDP coupe |

Serveurs locaux : chaque spec RDP démarre son propre serveur de test (aucun couplage) ;
un **sshd non-root** (port 2223, clé) est monté dans `onPrepare` pour `ssh.spec`.
En CI (`E2E_NO_RDP=1`), les specs à serveur local (ssh, rdp, rdp-reconnect) sont retirées.

## Astuces WebKitGTK

- `getText()` renvoie parfois vide → lire `getProperty("textContent")`.
- Le clic droit ne génère pas d'`contextmenu` → le dispatcher (`helpers.openCtx`).
- Les radios stylées ne sont pas « interactables » → cocher via `browser.execute` + event `change`.
