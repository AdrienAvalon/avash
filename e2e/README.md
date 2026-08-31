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

## Couverture (35 scénarios, 18 fichiers)

| Fichier | Ce qui est vérifié |
|---|---|
| `smoke.spec.js`       | démarrage, barre latérale, accueil |
| `hosts.spec.js`       | rendu des hôtes semés, dossier `prod`, sélection (`.picked`) |
| `hosts-move.spec.js`  | déplacer un hôte dans un dossier via « Déplacer vers… » |
| `folders.spec.js`     | cycle de vie complet : créer, sous-dossier, renommer, **supprimer** (modale maison), **annulation respectée** |
| `snippets.spec.js`    | snippet : créer, lister, **supprimer** (askConfirm) |
| `tunnels.spec.js`     | tunnel local : créer, lister, **supprimer** (askConfirm) |
| `a11y.spec.js`        | **accessibilité** : role=dialog + titre accessible, piège de focus (Tab ne fuit pas), focus rendu au déclencheur, noms accessibles des boutons icône |
| `isolation.spec.js`   | **garde-fou d'isolation** : chaque fichier part de l'état semé, sans reste des autres scénarios |
| `modals.spec.js`      | « Connexion directe » ne se ferme pas au clic dehors, se ferme à Échap ; palette Ctrl+K |
| `ssh.spec.js`         | **connexion SSH réelle** (sshd local, auth par clé) → session live |
| `sftp.spec.js`        | **panneau SFTP** sur la session SSH → listing du répertoire distant |
| `rdp.spec.js`         | **connexion RDP réelle** (serveur dédié) → handshake CredSSP + canvas (`.state.live`) |
| `rdp-clipboard.spec.js` | **presse-papiers RDP** (distant → poste) : pilote le sidecar sur son WebSocket, sans toucher au presse-papiers du système |
| `rdp-reconnect.spec.js` | **overlay de reconnexion** quand le serveur RDP coupe |
| `clavier.spec.js`     | palette aux flèches, `Ctrl+K` bloqué par-dessus une boîte, Échap ne ferme qu'une boîte à la fois |
| `liste-clavier.spec.js` | **barre latérale au clavier** : un seul arrêt de tabulation, flèches et Origine/Fin, focus qui vaut sélection, `Maj+F10` et navigation dans le menu |
| `prefs.spec.js`       | réglage du **partage de presse-papiers** : présent à la palette, bascule, retenu, libellé qui annonce l'état courant |
| `resize.spec.js`      | l'application reste répondante après une rafale de redimensionnements |

Chaque fichier de tests repart de l'état semé (`beforeSession` remet le bac à sable à zéro).
Serveurs locaux : chaque spec RDP démarre son propre serveur de test (aucun couplage) ;
un **sshd non-root** (port 2223, clé) est monté dans `onPrepare` pour `ssh.spec`.

En CI (`E2E_NO_RDP=1`), la configuration **exclut** les cinq scénarios qui
exigent un serveur local (`ssh`, `sftp`, `rdp`, `rdp-reconnect`,
`rdp-clipboard`). C'est une exclusion et non une énumération de ce qui tourne :
la liste énumérative prenait du retard à chaque scénario ajouté, et cinq
scénarios pourtant sans serveur ne tournaient plus qu'en local. Une nouvelle
spec sans serveur tourne désormais en intégration continue d'office.

L'attendu d'`isolation.spec.js` est **dérivé du semage** (`HOTES_SEMES`, exporté
par `wdio.conf.js`) : le réénoncer l'avait rendu faux en CI, où l'hôte
réellement joignable n'est pas semé.

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
