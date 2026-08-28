# Avash 😼 — le gestionnaire de connexions d'Ava

**Nom officiel validé par Adrien le 28/08 06:56.** *Ava + sh = avash.* Aussi : le ronronnement du chat heureux — ton terminal, content.

## Vision
Gestionnaire graphique de connexions : PuTTY/MobaXterm en mieux — **beau, simple, ultra rapide, sécurisé, multi-plateforme, révolutionnaire**.
- Natif (Tauri 2, pas d'Electron) : ~15 Mo, <100 Mo RAM, démarrage <1 s
- Zéro config : lit `~/.ssh/config` nativement
- Secrets chiffrés, aucune télémétrie
- Protocoles : SSH, SFTP, RDP (IronRDP), VNC, série, mosh (phase 2)
- Tueuses : multi-exécution, tunnels visuels, ProxyJump chaîné cliquable, édition distante, snippets, santé hôtes + WoL, enregistrement asciinema, import PuTTY/Moba
- DSL d'hôtes versionnable Git (v0.3+)

## Stack
- **Tauri 2** + xterm.js + russh (SSH pur Rust) + IronRDP
- Locale : `/home/avalon/dev/avash`

## État (28/08, après passe de validation)

- ✅ **36 tests verts** : 8 sur le cœur, 13 sur `avash-ui`, 15 sur `avash-web`.
- ✅ **Faille MITM corrigée** : une clé d'hôte modifiée est refusée, avec un
  message explicite remonté jusqu'à l'interface (il partait sur stderr, que
  personne ne lit dans une GUI). Couvert par `changed_host_key_is_refused`,
  vérifié comme échouant sur le code d'avant.
- ✅ **GUI opérationnelle** — binaire release de **15 Mo**, objectif tenu.
- ✅ **`./check.sh`** valide les trois projets d'une commande : compilation,
  tests, `cargo fmt`, `clippy -D warnings`, typage TS, build vite, release.
  `--quick` saute le build release.
- ✅ **CI GitHub Actions** sur les trois dépôts + hook `pre-commit` sur le cœur.

### Corrigé au passage

| Défaut | Effet |
|---|---|
| `select!` sur canal resize fermé | boucle à vide, 100 % CPU |
| `from_utf8_lossy` par bloc | accents corrompus dans le terminal |
| `pty_write` sur session inconnue | frappes perdues en silence |
| Collision d'identifiants de session | sortie d'un ancien serveur dans un nouvel onglet |
| Filtre palette ≠ filtre latéral | recherche par IP sans résultat |
| `file_name().unwrap_or_default()` | téléchargement vers le dossier lui-même |

## Feuille de route
- v0.1 (ce soir/aujourd'hui) : CLI + parseur + connexion russh → **GUI dès webkit installé**
- v0.2 : SFTP glisser-déposer, tunnels, snippets
- v0.3 : chiffrement secrets, imports, recherche instantanée
- v1 : RDP + multi-exécution + santé hôtes