# Politique de sécurité

## Signaler une vulnérabilité

Si tu découvres une vulnérabilité dans Avash, **ne l'ouvre pas dans une issue
publique**. Signale-la en privé via les **GitHub Security Advisories** du dépôt :

> Onglet **Security** → **Report a vulnerability**

À défaut, écris directement à **adrien.cros@outlook.com**.

Ce canal permet une divulgation coordonnée : le détail reste confidentiel
jusqu'à ce qu'un correctif soit disponible.

Merci d'inclure, dans la mesure du possible :

- une description du problème et de son impact ;
- les étapes de reproduction ;
- la version d'Avash et le système concerné.

### Délai de réponse

Il s'agit d'un projet à maintenance bénévole. Nous visons un **premier accusé
de réception sous 72 heures** et une évaluation initiale sous une semaine. Les
correctifs de sécurité sont priorisés par rapport aux autres travaux.

## Versions supportées

| Version | Supportée          |
|---------|--------------------|
| 0.2.x   | Oui                |
| < 0.2   | Non                |

Les correctifs de sécurité sont publiés pour la série **0.2.x**.

## Modèle de sécurité

Cette section décrit les protections **réellement implémentées** dans Avash.

### Stockage des secrets

Les mots de passe ne sont **jamais écrits en clair sur le disque**, ni dans
`~/.ssh/config` (qui est en clair et n'a d'ailleurs aucune directive pour un
mot de passe). Ils sont confiés au **trousseau du système** via la crate
`keyring` (voir `crates/avash/src/secrets.rs`) :

- Secret Service (KWallet / GNOME Keyring) sous Linux ;
- Gestionnaire d'identifiants sous Windows ;
- Trousseau sous macOS.

Le déverrouillage, le chiffrement et la révocation sont délégués au système.
Chaque entrée est identifiée de façon lisible (`user@hôte:port`), ce qui permet
de la retrouver et de la révoquer à la main dans l'outil du système.

### Mot de passe RDP transmis par stdin

Le mot de passe RDP est passé au sidecar `avash-rdp` **sur son entrée standard
(stdin)**, jamais en argument de ligne de commande (voir
`crates/avash-ui/src/rdp.rs` et `read_password` dans `rdp-sidecar/src/main.rs`).
Il n'apparaît donc **pas dans `/proc/<pid>/cmdline`** ni dans la liste des
processus, où tout utilisateur de la machine pourrait le lire.

### Canal local du sidecar RDP

Le flux entre l'application et le sidecar passe par un WebSocket qui écoute
**uniquement sur `127.0.0.1`**, sur un port éphémère. La première trame que le
sidecar attend est un **jeton aléatoire** généré à son démarrage : toute
connexion qui ne le présente pas est rejetée. Cela empêche un autre processus
local de se brancher sur la session.

### Vérification des clés d'hôte SSH (TOFU)

Avash vérifie la clé d'hôte SSH selon le modèle **TOFU (Trust On First Use)**,
avec la distinction que fait OpenSSH (voir `check_server_key` dans
`crates/avash/src/ssh.rs`) :

- **hôte inconnu** : la clé est apprise au premier contact et ajoutée à
  `~/.ssh/known_hosts` ;
- **clé identique** : connexion acceptée ;
- **clé changée** : la connexion est **refusée**, avec un message explicite
  (réinstallation du serveur ou interception possible) et l'empreinte SHA-256
  présentée. La clé n'est **jamais réapprise en silence** ;
- **`known_hosts` illisible, ou certificat d'hôte non validable** : refus par
  défaut (mieux vaut refuser qu'accepter à l'aveugle).

### Vérification du serveur RDP (TOFU)

La bibliothèque RDP accepte par construction n'importe quel certificat TLS : il
n'existe pas d'autorité de certification dans ce contexte. Avash applique donc
au RDP le même modèle qu'au SSH — l'empreinte SHA-256 de la **clé publique** du
serveur est mémorisée au premier contact dans `~/.config/avash/rdp_known_hosts`,
et toute clé différente fait **refuser la connexion**, avec les deux empreintes
affichées.

La vérification a lieu **avant CredSSP/NLA**, c'est-à-dire avant que le moindre
identifiant ne soit transmis. On épingle la clé plutôt que le certificat entier :
une reconduction de certificat à clé inchangée ne déclenche pas de fausse alerte.

Si le changement est légitime (serveur réinstallé), retirer la ligne
correspondante de `rdp_known_hosts`.

### Protection contre l'injection dans `~/.ssh/config`

Avash écrit et relit `~/.ssh/config`. Avant toute écriture, les champs d'un
hôte sont validés (voir `validate_host` / `validate_config_value` dans
`crates/avash/src/lib.rs`) :

- tout caractère de saut de ligne (`\n`, `\r`) ou nul (`\0`) dans un champ est
  **rejeté** — sans quoi une valeur piégée pourrait injecter une directive
  arbitraire (par exemple `ProxyCommand`) ;
- le nom d'hôte ne peut contenir ni espace, ni joker (`*`, `?`, `!`), qui
  s'appliqueraient à d'autres connexions.

Les noms de dossiers sont normalisés de la même façon (segments contenant un
saut de ligne retirés — voir `crates/avash/src/folders.rs`).

### Aucune télémétrie

Avash **ne collecte et n'envoie aucune donnée d'usage**. Les seules connexions
réseau sont celles que tu inities (SSH, RDP, SFTP, tunnels) et, si tu cliques
sur la pastille de version, la vérification du manifeste de mise à jour Tauri.
