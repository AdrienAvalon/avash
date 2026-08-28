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

## État (29/08)

**72 tests verts** · clippy strict · `cargo audit` sans vulnérabilité non
justifiée · binaire **13 Mo** · démarrage **0,17 s**.

### Chiffres mesurés face aux objectifs

| Objectif spec | Visé | Mesuré | |
|---|---|---|---|
| Démarrage | < 1 s | **0,17 s** | ✅ |
| Binaire | ~15 Mo | **13 Mo** | ✅ |
| RAM | < 100 Mo | **297 Mo (PSS)** | ❌ |

⚠️ **L'objectif de 100 Mo n'est pas atteignable** avec une architecture à
webview. Décomposition mesurée en PSS (le RSS, à 450 Mo, surestime : il
compte plusieurs fois des bibliothèques partagées avec le reste du bureau) :

| | PSS | privé |
|---|---|---|
| `WebKitWebProcess` | 159 Mo | 125 Mo |
| `avash-ui` | 107 Mo | 73 Mo |
| `WebKitNetworkProcess` | 31 Mo | 22 Mo |

Les 280 Mo de WebKit sont le plancher de la plateforme. Le chiffre de la
spec vient d'une comparaison à Electron (~400 Mo), pas d'une mesure — il
reste vrai qu'Avash consomme bien moins, mais pas moins de 100 Mo.

### Sécurité

- Clé d'hôte modifiée **refusée**, avec un message exploitable (couvert par
  un test vérifié comme échouant sur le code d'avant le correctif).
- `russh 0.63` : deux failles 7.5 corrigées.
- Seule faille restante : `RUSTSEC-2023-0071` (attaque Marvin sur `rsa`,
  5.9, sans correctif amont). Conservée sciemment — la désactiver casse les
  serveurs à clé d'hôte RSA et les clés `id_rsa`. Arbitrage détaillé dans
  `crates/avash/Cargo.toml` et `audit.toml`.

### Performance

- Décodeur UTF-8 : **1,5 Go/s** — hors du chemin critique, aucune
  optimisation nécessaire (mesuré avant de toucher quoi que ce soit).
- Sorties regroupées sur 8 ms ou 16 Ko avant émission : **92 % de messages
  IPC en moins** sur une trace réelle. Les blocs SSH font souvent 1 à
  100 octets, et chacun coûtait sinon un message JSON, un aller-retour IPC
  et une écriture xterm.

### Validation

    ./check.sh            # tout : tests, format, clippy, audit, build
    ./check.sh --quick    # sans le build release

Un hook `pre-commit` rejoue format, clippy et tests avant chaque commit.

## Feuille de route
- v0.1 (ce soir/aujourd'hui) : CLI + parseur + connexion russh → **GUI dès webkit installé**
- v0.2 : SFTP glisser-déposer, tunnels, snippets
- v0.3 : chiffrement secrets, imports, recherche instantanée
- v1 : RDP + multi-exécution + santé hôtes