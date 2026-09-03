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
| 0.6.x   | Oui                |
| < 0.6   | Non                |

Les correctifs de sécurité sont publiés pour la série **0.6.x**, la dernière
publiée ; une version antérieure se met à jour par la mise à jour automatique
ou depuis la page des releases.

> **La 0.3.0 a corrigé plusieurs défauts sérieux des séries antérieures** — clé
> d'hôte SSH insuffisamment vérifiée, certificat RDP pas vérifié du tout, repli
> de NLA vers TLS accepté — la 0.6.1 durcit le collage, la webview et le
> processus RDP, et la 0.6.2 cesse de garder le mot de passe en mémoire une
> fois la session établie. Une mise à jour depuis toute version antérieure est
> vivement recommandée. Le détail est dans le [CHANGELOG](CHANGELOG.md).

## Modèle de sécurité

Cette section décrit les protections **réellement implémentées** dans Avash.

### Modèle de menace

Contre qui Avash se défend, et contre qui il ne prétend pas se défendre. Les
sections suivantes détaillent chaque protection ; celle-ci dit à quoi elles
répondent.

**Adversaires pris en compte :**

- **Quelqu'un sur le chemin réseau** entre vous et le serveur, capable de lire,
  modifier ou se substituer au serveur. Réponses : vérification de la clé
  d'hôte SSH et de l'empreinte du certificat RDP dès le premier contact, avec
  refus explicite au changement ; NLA exigé en RDP, sans repli silencieux vers
  TLS seul ; aucun secret ne quitte la machine autrement que par le protocole
  chiffré négocié.
- **Un serveur malveillant, compromis ou simplement défaillant.** Une fois la
  session établie, tout ce qu'il envoie est traité comme une entrée non fiable :
  tailles bornées, rectangles rognés, décodeurs graphiques exercés par fuzzing à
  partir de trafic réel, redirections plafonnées. Le pire qu'il puisse obtenir
  est une session qui se ferme, jamais une écriture hors de l'image ni une
  exécution de code.
- **Un autre compte sur votre machine.** Fichiers d'état, empreintes, clés
  générées et traces sont créés en 0600 dans un répertoire en 0700 ; le canal
  entre l'interface et le processus RDP n'écoute que sur l'adresse locale, avec
  un jeton d'une seule vie et un contrôle d'origine ; le mot de passe RDP passe
  par l'entrée standard, jamais par la ligne de commande visible de tous.
- **Du contenu hostile arrivant dans le terminal ou le presse-papiers.** Un
  collage de plusieurs lignes est montré avant d'être envoyé ; la webview
  n'exécute que le code embarqué (`default-src 'self'`, pas de contenu distant),
  et ne dispose que des commandes déclarées dans `capabilities/default.json`.
- **Un fichier `~/.ssh/config` édité à la main ou fourni par un tiers.** Le
  parseur refuse ce qui injecterait une directive, et les écritures sont
  atomiques.

**Hors du modèle :**

- **Un attaquant qui contrôle déjà votre compte ou la machine** (racine, autre
  processus de votre session, débogueur). Il lit le trousseau, la mémoire et
  l'entrée standard comme Avash le fait.
- **Le trousseau du système lui-même.** Avash s'en remet à lui pour les mots de
  passe ; sa protection est celle de votre session ouverte.
- **Les traces de diagnostic RDP**, activées à la demande : elles contiennent
  tout le trafic, mot de passe compris, et sont à traiter comme tel.
- **Les canaux auxiliaires** (durée des opérations, taille des trames
  chiffrées) et **l'accès physique** à une session déverrouillée.
- **La chaîne d'approvisionnement au-delà de ce qui est vérifié** : `cargo
  audit`, `cargo deny`, versions épinglées et attestation Sigstore des binaires
  publiés, mais aucune revue indépendante de chaque dépendance.

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
`crates/avash-ui/src/rdp.rs` et `read_password` dans `rdp-sidecar/src/args.rs`).
Il n'apparaît donc **pas dans `/proc/<pid>/cmdline`** ni dans la liste des
processus, où tout utilisateur de la machine pourrait le lire.

### Canal local du sidecar RDP

Le flux entre l'application et le sidecar passe par un WebSocket qui écoute
**uniquement sur `127.0.0.1`**, sur un port éphémère. La première trame que le
sidecar attend est un **jeton aléatoire** de 64 bits généré à son démarrage :
toute connexion qui ne le présente pas est rejetée. Cela empêche un autre
processus local de se brancher sur la session.

Le sidecar **boucle** sur les connexions entrantes plutôt que d'en accepter une
seule — voir plus bas, « Le canal local du processus RDP n'est plus coupable
d'un seul message ».

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

**L'algorithme n'entre pas en compte dans la décision**, et c'est le point le
plus important. Le booléen de `check_known_hosts` de russh répond « hôte
inconnu » lorsque l'algorithme de la clé présentée diffère de celui enregistré :
s'y fier revenait à traiter une clé changée comme un premier contact, et à la
réapprendre en silence. Avash lit lui-même les clés enregistrées et tranche dans
`juger_cle_hote`.

**Les marqueurs d'OpenSSH font refuser la connexion.** La correspondance d'hôte
de russh est une simple égalité de chaîne : une ligne `@revoked srv …` était
découpée en hôte « @revoked », qui ne correspond à rien — la clé que vous aviez
marquée comme compromise aurait été réapprise et acceptée. Avash refuse
`@revoked` et `@cert-authority` avec un message qui dit pourquoi : nous ne
savons pas valider une autorité de certification, et faire semblant serait pire
que refuser.

### Le processus RDP n'est jamais cherché en chemin relatif

Le chemin de repli du processus `avash-rdp` était `rdp-sidecar/target/release/`,
résolu depuis le **répertoire courant**. Lancée depuis `/tmp`, un partage ou
`~/Téléchargements`, l'application y exécutait le binaire qu'un autre compte
avait pu déposer — et lui écrivait le mot de passe RDP sur son entrée standard,
celui du trousseau compris. Le repli a été supprimé : à côté de l'exécutable, un
chemin absolu de développement, ou une erreur nommée. `AVASH_RDP_BIN` n'est
honorée que si elle est absolue.

### Rien d'illimité ne vient du réseau

Trois allocations n'avaient aucun plafond, toutes pilotables par un serveur :

- la sortie d'une commande distante était accumulée sans borne. La sonde d'OS
  part à **chaque ouverture d'onglet** : un serveur répondant `cat /dev/zero`
  faisait tomber Avash entier, avec tous ses autres onglets, tunnels et
  transferts. Plafond : 1 Mio, la troncature est annoncée.
- la résolution annoncée par un serveur RDP était allouée telle quelle
  (`largeur × hauteur × 4`), soit 17 Gio pour un 65535×65535, rejouable à
  volonté par renégociation. Plafond : 8192×8192.
- le presse-papiers reçu du serveur était déjà borné à 8 Mio.

### Le canal local du processus RDP n'est plus coupable d'un seul message

Le port d'écoute du processus RDP s'ouvre avant que l'interface n'en soit
avertie. Il n'acceptait **qu'une** connexion, et un premier message autre que le
jeton le faisait quitter : n'importe quel processus local — ou une page web, les
WebSocket n'étant pas soumises à la politique d'origine pour *établir* la
connexion — détruisait ainsi une session RDP déjà authentifiée. Une connexion
laissée sans poignée de main consommait la seule place disponible et l'interface
n'arrivait jamais à se connecter. L'intrus est désormais rejeté, la file
continue, et chaque tentative a un délai de garde. Le jeton (64 bits) n'a jamais
été à portée : c'était un déni de service, pas un détournement.

### Écritures atomiques

`~/.ssh/config`, `known_hosts`, `rdp_known_hosts` et les quatre fichiers de
configuration sont écrits dans un temporaire créé en 0600 dans le même
répertoire, puis renommés. Auparavant la troncature précédait l'écriture : une
coupure laissait un fichier vide — pour `~/.ssh/config`, **toute** la
configuration SSH de l'utilisateur ; pour `rdp_known_hosts`, la perte des
empreintes, donc le retour silencieux à « premier contact » pour tous les
serveurs. Le temporaire naissait par ailleurs avec l'umask, laissant
`snippets.yaml` — qui contient des commandes d'administration — brièvement
lisible par les autres comptes.

Une adresse de bureau RDP contenant une espace ou un saut de ligne est refusée :
le fichier d'empreintes se découpe au premier espace, et une telle adresse
produisait une ligne jamais retrouvée — TOFU neutralisé sans que rien ne le dise.

### Surface d'appel réduite

Trois commandes exposées à l'interface n'étaient appelées par aucun code :
`run_command`, `snippet_vars` et `password_known`. La première exécutait une
commande arbitraire sur n'importe quel alias, avec le mot de passe du trousseau
chargé automatiquement. Elles ne sont plus enregistrées.

### Le presse-papiers n'est partagé que sur décision

Partager le presse-papiers avec un bureau distant revient à confier son contenu
— souvent un mot de passe qu'on vient de copier — à un serveur qui peut le
réclamer dès qu'on le lui annonce. Avash l'annonçait **au simple retour sur sa
fenêtre**, donc à chaque bascule d'application, à tout bureau ouvert.

Ce n'est plus le cas : le presse-papiers ne part que depuis la session active,
sur un geste dans le bureau distant. Et le partage est révocable — `Ctrl+K`,
« Ne plus partager le presse-papiers avec les bureaux RDP » — le choix étant
retenu d'un lancement à l'autre.

Le réglage vaut dans les **deux** sens. Il ne gardait que le sens sortant, alors
qu'un bureau distant pouvait remplacer en boucle le presse-papiers du poste : on
copie une commande depuis sa documentation, on colle dans son terminal local, on
exécute celle de l'attaquant. Refusé, le partage n'est plus ni annoncé ni
réclamé au serveur — le processus RDP en est informé, pas seulement l'interface.

### Les mots de passe ne traversent pas l'interface

Le mot de passe d'un bureau enregistré est lu par le cœur natif au moment de la
connexion. L'interface ne le demande pas au trousseau et ne le reçoit jamais :
elle interroge seulement son existence. Le volet SSH procède ainsi depuis
toujours ; le volet RDP rapatriait le secret dans la webview, où il séjournait
toute la durée de l'onglet.

### Authentification par PAM : ce à quoi nous répondons, et ce à quoi nous refusons de répondre

Quand un serveur confie l'authentification à PAM (`keyboard-interactive`), il
pose des questions et le client y répond. Avash répond avec le mot de passe déjà
saisi **uniquement aux invites masquées** — celles dont la réponse ne s'affiche
pas, c'est-à-dire un mot de passe.

Une invite **en clair** n'est pas un mot de passe : c'est un code à usage unique,
une question de sécurité, un choix de second facteur. Y envoyer le mot de passe
le livrerait tel quel au serveur, qui l'afficherait, et n'aboutirait pas. Avash
renonce alors en citant l'invite, plutôt que de tenter à l'aveugle.

L'authentification à plusieurs facteurs n'est donc **pas encore prise en
charge** — c'est une limite connue, pas un oubli silencieux.

### Pas de repli de NLA vers TLS seul (RDP)

C'est le serveur qui choisit le protocole de sécurité parmi ceux que le client
annonce. Annoncer `PROTOCOL_SSL` revient — la documentation d'IronRDP le dit mot
pour mot — à lui signifier qu'on **accepte de renoncer à NLA**. Un serveur
répondant « SSL seul » faisait alors sauter CredSSP, et le mot de passe partait
dans le *Client Info PDU*, sans authentification mutuelle du serveur.

Avash n'annonce que `HYBRID` : un serveur incapable de NLA fait échouer la
négociation. C'est au **premier contact** — le seul moment où l'épinglage
ci-dessous ne protège pas encore — que cette différence compte le plus.

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

### `AVASH_HOME` : où Avash cherche votre configuration

Par défaut, Avash lit `~/.ssh` et sa propre configuration à l'emplacement que le
système désigne comme répertoire personnel. La variable d'environnement
`AVASH_HOME`, quand elle est posée, prend le pas — c'est ce qui permet aux tests
de s'isoler du vrai profil, y compris sous Windows où le remplacement de `HOME`
n'a aucun effet.

Ce n'est pas une porte dérobée : qui peut poser une variable d'environnement
dans le processus peut déjà bien davantage (précharger une bibliothèque,
détourner le `PATH`). C'est documenté ici parce qu'une variable qui change
l'endroit d'où l'on lit des clés mérite d'être connue, pas cachée.

### Aucune télémétrie

Avash **ne collecte et n'envoie aucune donnée d'usage**. Les seules connexions
réseau sont celles que tu inities (SSH, RDP, SFTP, tunnels) et, si tu cliques
sur la pastille de version, la vérification du manifeste de mise à jour Tauri.

### Les traces de diagnostic contiennent le mot de passe

Le processus RDP sait raconter la séquence de connexion — c'est ce qui a permis
de localiser une connexion qui restait suspendue. Ces traces **contiennent le
mot de passe en clair** : la requête CredSSP le transporte encodé en UTF-16,
lisible tel quel dans la sortie.

Deux décisions en découlent :

- elles ne s'activent **pas** sur `RUST_LOG`, que beaucoup exportent
  globalement, mais sur `AVASH_RDP_TRACE`, qui n'appartient qu'à nous. Ce qui
  sert à trouver un défaut ne doit pas s'allumer par accident ;
- leur activation affiche un avertissement sur la sortie d'erreur, pour que
  personne ne colle une trace dans un ticket sans l'avoir relue.

### Chaîne d'approvisionnement : au-delà des vulnérabilités déclarées

`cargo audit` ne voit que ce qui est déclaré dans la base RustSec. Trois autres
portes restaient ouvertes, et le projet embarque désormais du code tiers modifié
(`rdp-sidecar/vendor`), ce qui rend la question plus vive :

- **licences** — une dépendance transitive sous une licence incompatible
  arrivait sans que rien ne le signale ;
- **dépendances en joker** (`version = "*"`) — elles rendent la construction non
  reproductible : deux compilations du même commit peuvent embarquer des codes
  différents ;
- **sources** — une dépendance tirée d'ailleurs que le registre officiel doit
  être un choix explicite, jamais une surprise.

`cargo deny check licenses bans sources` couvre les trois, sur les **deux**
arbres de dépendances (workspace et processus RDP), dans `check.sh` comme dans
les deux chaînes d'intégration continue. La politique est dans `deny.toml`, et
chaque exception y est justifiée en clair.

**Fuzzing.** Ce que le cœur lit depuis un fichier écrit par quelqu'un d'autre
(`~/.ssh/config`, sessions PuTTY et MobaXterm, registre, enregistrements
asciicast) passe sous cargo-fuzz — génération guidée par la couverture, sur
nightly — dans `fuzz/`, à chaque poussée et chaque lundi. La règle est la
même que pour le processus RDP : aucune entrée ne doit faire paniquer, et ce
qui est accepté doit rester cohérent (alias non vide, port non nul, rebond
rogné). Voir `fuzz/README.md`.

**Ce que dit Scorecard, et pourquoi.** L'OpenSSF Scorecard, joué chaque lundi,
compte « 36 vulnérabilités » sur le dépôt. Ce sont les avis RustSec
*non maintenu* de la pile GTK 3 (`atk`, `gdk-sys`, `gdkwayland-sys`… :
RUSTSEC-2024-0411 à 0418) que Tauri embarque sous Linux — pas des failles, et
pas un choix qui nous appartient : le correctif viendra avec le passage de
Tauri à GTK 4. `cargo deny` les connaît (« unmaintained = workspace » ne bloque
que nos dépendances directes) et l'alerte reste ouverte à dessein plutôt que
masquée. Ses autres constats relèvent des réglages du dépôt, pas du code. La
branche `main` est protégée depuis le 3 septembre 2026, sur GitHub comme sur
GitLab : ni poussée forcée ni suppression, mainteneur compris, sans exiger de
PR — un mainteneur seul pousse directement. La revue de code obligatoire et le
badge de bonnes pratiques supposent l'un un flux par PR, l'autre une
inscription sur bestpractices.dev : ils restent des choix du mainteneur.

### Un serveur RDP est une entrée non fiable

Le modèle de sécurité d'avash traitait le serveur comme un pair : on vérifie son
identité (TOFU sur la clé publique), on refuse de dégrader NLA sans décision
explicite. Mais une fois la session établie, tout ce qu'il envoie était décodé
avec confiance.

Or rien n'oblige un serveur à être correct. Il peut être malveillant, compromis,
ou simplement abîmé. Un **fuzzing par mutation à partir d'un enregistrement
réel** (voir `rdp-sidecar/src/magnetoscope.rs`) a mis au jour deux défauts par
lesquels un serveur faisait tomber le client :

- une **écriture hors du tampon d'image** quand il annonce plus de lignes que le
  rectangle n'en contient — six chemins de décodage ne bornaient rien ;
- un **débordement arithmétique** sur un rectangle dont les bords sont dans le
  désordre, la garde existante ne vérifiant que l'appartenance à l'image.

Rust arrête ces écritures : il n'y a pas de corruption mémoire, donc pas
d'exécution de code. Mais le processus meurt, emportant une session établie —
un déni de service à distance, déclenchable par le seul serveur auquel on se
connecte.

Les deux sont corrigés dans `rdp-sidecar/vendor/` et couverts par une campagne
de fuzzing qui tourne à chaque vérification. La leçon dépasse ces deux cas :
**muter du trafic authentique atteint des chemins que des octets aléatoires
n'atteignent jamais**, parce que les premières validations rejettent tout ce qui
ne ressemble pas à du RDP.
