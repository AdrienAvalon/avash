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

- ✅ **7 tests verts** : 2 unitaires (parseur `~/.ssh/config`) + 5 d'intégration
  (connexion+exec, PTY write/resize, SFTP list/download/upload, et deux tests
  de non-régression sur la vérification de clé d'hôte).
- ✅ **Faille MITM corrigée** : `check_server_key` distinguait mal « hôte
  inconnu » et « clé changée » et réapprenait la clé dans les deux cas. Une
  clé d'hôte modifiée est désormais refusée. Couvert par
  `changed_host_key_is_refused`, vérifié comme échouant sur le code d'avant.
- ✅ **GUI opérationnelle** : `webkit2gtk-4.1` installé, `cargo build --release`
  produit un binaire de **15 Mo** (objectif tenu), la fenêtre se lance.
- ⚠️ **`avash-ui` n'a aucun test** — les 9 commandes Tauri sont non couvertes.
- ⚠️ `avash-ui/target` pèse ~7,5 Go (`cargo clean` pour récupérer l'espace).

## Feuille de route
- v0.1 (ce soir/aujourd'hui) : CLI + parseur + connexion russh → **GUI dès webkit installé**
- v0.2 : SFTP glisser-déposer, tunnels, snippets
- v0.3 : chiffrement secrets, imports, recherche instantanée
- v1 : RDP + multi-exécution + santé hôtes