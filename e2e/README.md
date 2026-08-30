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

## Couverture

| Fichier | Ce qui est vérifié |
|---|---|
| `smoke.spec.js`   | démarrage, barre latérale, accueil |
| `hosts.spec.js`   | rendu des hôtes semés, dossier `prod`, sélection (`.picked`) |
| `folders.spec.js` | cycle de vie complet : créer, sous-dossier, renommer, **supprimer** (modale de confirmation maison), **annulation respectée** |
| `modals.spec.js`  | « Connexion directe » ne se ferme pas au clic dehors, se ferme à Échap ; palette Ctrl+K |
| `rdp.spec.js`     | **connexion RDP réelle** au serveur de test → handshake CredSSP + canvas (`.state.live`) |

## Astuces WebKitGTK

- `getText()` renvoie parfois vide → lire `getProperty("textContent")`.
- Le clic droit ne génère pas d'`contextmenu` → le dispatcher (`helpers.openCtx`).
- Les radios stylées ne sont pas « interactables » → cocher via `browser.execute` + event `change`.
