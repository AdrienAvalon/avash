# Tests E2E (bout en bout)

Pilote la **vraie application compilée** dans son runtime réel (WebKitGTK via
`tauri-driver`). C'est le seul niveau qui attrape les bugs spécifiques au runtime
(ex. `window.prompt()` inopérant sous WebKitGTK) et les flux utilisateur complets
— là où vivaient la plupart des bugs de détail.

## Prérequis
- `tauri-driver` : `cargo install tauri-driver --locked`
- `WebKitWebDriver` : fourni par le paquet **webkitgtk-6.0** (Arch/CachyOS :
  `sudo pacman -S webkitgtk-6.0`). Sur Debian/Ubuntu : `webkit2gtk-driver`.
- Le binaire release : `cargo build --release -p avash-ui` (à la racine).

## Lancer
```
cd e2e && npm install       # une fois
npm test
```

Isolation : l'app tourne avec un `HOME`/`XDG_CONFIG_HOME` temporaires — aucun hôte
réel, registre de dossiers vierge, **zéro effet sur ta vraie config**.

Les tests ouvrent de vraies fenêtres : à lancer quand tu n'utilises pas activement
le poste, ou en CI avec `xvfb-run`.

## Ajouter un scénario
Un fichier `specs/<flux>.spec.js`. API WebdriverIO (`$`, `$$`, `browser`, `expect`).
Astuce : lire le texte d'un nœud avec `getProperty("textContent")` (le `getText()`
de l'automation WebKitGTK renvoie parfois vide).
