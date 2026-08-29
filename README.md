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

## État (29/08 — tunnels SSH)

**123 tests verts** (95 Rust, 28 TypeScript) · clippy strict · `cargo audit`
sans vulnérabilité non justifiée · démarrage 0,17 s.

### Tunnels SSH (nouveau)

Les trois redirections d'OpenSSH, visuelles : **`-L`** (local → serveur →
destination), **`-R`** (serveur → local → destination) et **`-D`** (mandataire
SOCKS5 sortant par le serveur). Menu contextuel d'un hôte → *Tunnels…*, ou
bouton *Tunnels* de la barre latérale.

- Chaque tunnel vit sur **sa propre connexion SSH** : fermer un onglet ne le
  coupe pas, et inversement. Keepalive 30 s (3 échecs → tunnel marqué
  « connexion perdue », bouton *Relancer*).
- Compteurs en direct : connexions en cours, octets ↑/↓ — mis à jour pendant
  la connexion, pas seulement à sa fin (une session VNC ou Postgres dure).
- Définitions dans `~/.config/avash/tunnels.yaml` (écriture atomique).
- Écoute **loopback uniquement** ; un `-R` ne relaie que vers la destination
  déclarée (un serveur malveillant ne peut pas faire ouvrir une connexion
  locale arbitraire). SOCKS : CONNECT seul, sans authentification, SOCKS4
  refusé.
- Vérifié contre **OpenSSH 10.5 réel** avec `examples/tunnel_probe.rs` : les
  trois types relaient, le port `-R` est libéré à la fermeture.

### Bugs de correction trouvés pendant l'audit

| Défaut | Conséquence | Détecté par |
|---|---|---|
| `run()` cassait sur `Eof` | **code de sortie toujours 0** — un déploiement de clé raté passait pour réussi | test contre un vrai sshd |
| serveur de test : `exit-status` avant `eof` | masquait le bug ci-dessus | audit de l'ordre réel |
| `Match` non reconnu | directives attribuées au mauvais hôte — mauvais user/port silencieux | relecture du parseur |
| `pty_close` : `into_inner().unwrap()` | plantait à la fermeture si un mutex était empoisonné | audit des paniques |

### Chemins de sécurité, chacun couvert par un test

- Clé d'hôte modifiée refusée (vérifié comme échouant sur le code d'avant)
- Injection shell dans le déploiement de clé refusée
- Injection de directive via un alias `~/.ssh/config` refusée
- Traversée de chemin au téléchargement et dans un nom de clé refusée
- Traversée SFTP (`..`) impossible via `parentDir`
- Mot de passe absent des traces (`Debug` masquant, testé)
- `Match` ne contamine plus l'hôte précédent
- `Include` circulaire borné à 16 niveaux
- Code de sortie fiable (non-régression)

**Chemin non testé, assumé** : le refus d'un certificat d'hôte SSH. Simuler
un serveur à certificat signé demande une infrastructure lourde ; le chemin
est en échec sécurisé (il refuse), et le défaut de `russh` refuse aussi.

### Vérifié contre un vrai serveur, pas un simulacre

Les bugs les plus pénibles (terminal muet, code de sortie, ordre des messages
SSH) ne se voyaient qu'en conditions réelles. `examples/pty_probe.rs`,
`examples/tunnel_probe.rs` et `examples/keyring_check.rs` sont conservés comme outils : ils testent contre le
`sshd` et le trousseau réels de la machine, là où un simulacre ment.

### Objectifs de la spec, mesurés

| | Visé | Mesuré | |
|---|---|---|---|
| Démarrage | < 1 s | 0,17 s | ✅ |
| RAM | < 100 Mo | 297 Mo (PSS) | ❌ plancher WebKit |

L'objectif de 100 Mo n'est pas atteignable avec une webview ; voir plus bas.

## Feuille de route
- v0.1 (ce soir/aujourd'hui) : CLI + parseur + connexion russh → **GUI dès webkit installé**
- v0.2 : ~~tunnels~~ ✅, SFTP glisser-déposer, snippets
- v0.3 : chiffrement secrets, imports, recherche instantanée
- v1 : RDP + multi-exécution + santé hôtes