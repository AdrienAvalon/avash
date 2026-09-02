# Feuille de route

Ce document fixe le cap d'avash et sert de point de reprise entre les sessions de
travail. Il est volontairement **fondé sur des constats mesurés**, pas sur des
intentions : chaque objectif est vérifiable.

Dernière révision : 2 septembre 2026, après la publication de la version 0.6.1
— un lot de durcissement (collage, WebView2, traces RDP, WebSocket) et de
corrections, après la 0.6.0 qui avait étendu le pipeline graphique RDP à
Windows.

Dépôts : [GitHub](https://github.com/AdrienAvalon/avash) (public) · GitLab interne (privé).

---

## Le cap

> Le gestionnaire de connexions qu'un administrateur système ouvre le matin et
> ne referme pas : rapide, sûr, et qui ne surprend jamais.

Trois exigences, dans cet ordre, quand elles s'opposent :

1. **Sûreté** — un secret ne fuit pas, une action destructrice demande confirmation,
   une clé d'hôte qui change bloque la connexion.
2. **Justesse** — ce que l'interface affiche correspond à l'état réel. Pas de faux
   « connecté », pas de bouton qui ne fait rien.
3. **Vitesse** — démarrage immédiat, aucune latence perceptible à la frappe.

Le confort passe après. Une fonctionnalité qui met l'une de ces trois exigences en
défaut n'est pas livrée, même terminée.

---

## Où nous en sommes (mesuré)

| Indicateur | Valeur au 02/09/2026 |
|---|---|
| Tests | 289 Rust (118 cœur, 32 intégration, 53 interface, 86 processus RDP) · 387 dans les paquets IronRDP portés · 90 front · 40 scénarios bout en bout dans 21 fichiers, tous en intégration continue |
| Binaire Linux | 18 Mo (`codegen-units=1`, LTO fin) ; AppImage publiée 85 Mo |
| Paquet front | 572 Ko en un seul module |
| Plateformes livrées | Linux (AppImage) et Windows (NSIS + portable), éprouvées sur machine réelle |
| Dette déclarée | aucun `TODO`/`FIXME` dans le code |
| Version publiée | 0.6.1 (Linux + Windows, signées, attestation Sigstore) |
| Licence | AGPL-3.0-or-later (+ licence commerciale possible) |

Acquis récents : Windows validé en usage réel (RDP, clavier, mise à jour
automatique), gel au redimensionnement corrigé après profilage, et deux cycles
d'audits dont les constats sont traités ci-dessous.

---

## Axe 1 — Fiabiliser ce qui existe

*Priorité haute. Rien de neuf tant que ces points ne sont pas réglés.*

### 1.1 Réparer la mise à jour automatique — **fait**

Trois causes distinctes, découvertes l'une après l'autre : l'adresse consultée
visait un dépôt inexistant, aucun manifeste `latest.json` n'était publié, et le
bundler ne signait rien faute de `bundle.createUpdaterArtifacts`. Les trois sont
corrigées, la clé de signature est déposée en secret du dépôt, et la chaîne a été
**éprouvée en conditions réelles** : une version installée détecte la suivante,
la télécharge et redémarre.

### 1.2 Construire réellement pour Windows — **fait**

Le workflow a été exercé : **l'installeur Windows (8,2 Mo) et l'AppImage Linux
(81 Mo) sont produits**. L'exercice a révélé trois défauts qu'une machine de
développement Linux masquait :

1. le projet ne compilait pas depuis un clone neuf (binaire du sidecar RDP absent) ;
2. **avash ne compilait pas du tout sous Windows** — l'authentification par agent
   utilisait une API Unix (`SSH_AUTH_SOCK`). La promesse « multi-plateforme »
   était fausse depuis le début ;
3. le bundle Tauri s'exécutait depuis le mauvais répertoire en release.

Tous corrigés, avec un garde-fou d'intégration continue qui vérifie la
compilation Windows du cœur à chaque poussée.

**Éprouvé depuis** sur une machine réelle, ce qui a révélé quatre défauts de plus,
tous corrigés : une fenêtre de console s'ouvrait à côté de chaque bureau RDP
(`CREATE_NO_WINDOW` manquant), l'antislash et le pavé numérique ne passaient pas,
le verrouillage numérique était inversé entre le poste et le distant, et le
sidecar n'était pas trouvé faute d'extension `.exe`. Une version portable
accompagne désormais l'installeur.

### 1.3 Isoler l'état entre les fichiers de test — **fait**

Les scénarios partageaient un seul bac à sable : un fichier qui créait un dossier
ou déplaçait un hôte faussait les suivants. Corrigé — le bac à sable est remis à
son état semé avant chaque fichier, et un garde-fou (`isolation.spec.js`) le
vérifie. Il a été vu échouer avant correction, conformément à la règle.

---

## Les deux cycles d'audits du 31 août — ce qu'ils ont trouvé

Quatre audits parallèles (fiabilité, sécurité, performance, ergonomie) sur
l'ensemble du code. Trente-huit constats, dont ceux-ci, tous corrigés et chacun
accompagné d'un test vu échouer :

**Deux failles graves, du même genre** — nous ne vérifiions pas à qui nous
parlions. La clé d'hôte SSH était jugée par `check_known_hosts` de russh, qui
répond « hôte inconnu » quand seul l'algorithme diffère : une clé changée passait
donc pour un premier contact. Et le volet RDP n'examinait aucun certificat, la
bibliothèque installant `NoCertificateVerification` — les identifiants partaient
ensuite par CredSSP. Les deux sont réglées, vérifiées en substituant réellement
la clé du serveur de test.

**Un chemin relatif exécutable.** Le processus RDP était cherché en dernier
recours dans `rdp-sidecar/target/release/`, résolu depuis le répertoire courant ;
lancée depuis un répertoire partagé, l'application y aurait exécuté n'importe
quel binaire déposé, et lui aurait écrit le mot de passe RDP sur l'entrée
standard.

**Des allocations sans plafond**, toutes pilotables par un serveur distant : la
sortie d'une commande (la sonde d'OS part à chaque ouverture d'onglet) et la
résolution annoncée par un serveur RDP.

**Des écritures qui tronquaient avant d'écrire** — dont `~/.ssh/config`, qui
n'appartient pas qu'à nous.

**Le téléchargement SFTP**, huit fois plus lent que le téléversement faute de
requêtes en vol, et qui détruisait le fichier local avant de savoir s'il
aboutirait.

**La barre latérale**, entièrement hors d'atteinte au clavier.

**Le second cycle**, lancé sur l'état corrigé, avait pour première mission de
vérifier ce que le premier avait cassé. Il a trouvé cinq régressions de notre
main, dont deux graves : l'écriture atomique remplaçait les **liens
symboliques** (une configuration de dotfiles serait devenue silencieusement
orpheline), et le téléchargement en bandes pouvait promouvoir un fichier
**troué** en le déclarant réussi. Il a aussi trouvé le pire du lot côté RDP —
**le repli de NLA vers TLS était accepté**, donc un serveur pouvait obtenir le
mot de passe sans authentification mutuelle.

Et un constat d'un autre ordre : **les tests du processus RDP ne s'exécutaient
nulle part**. Hors du workspace, ils échappaient à `cargo test --workspace`, et
l'intégration continue se contentait de le compiler. Les trois tests du TOFU de
certificat n'avaient jamais tourné depuis leur écriture.

Ce que l'exercice enseigne : les défauts les plus graves n'étaient pas dans le
code neuf, mais dans les hypothèses tacites — « la bibliothèque vérifie », « ce
chemin n'arrive jamais », « le serveur est honnête », « les tests tournent ».

---

## Axe 2 — Élargir la portée

### 2.1 macOS

Ni construit ni testé. C'est la dernière plateforme majeure manquante pour tenir
la promesse « multi-plateforme ».

### 2.2 Anglais

L'interface est entièrement en français, chaînes écrites en dur (28 affectations
littérales rien que dans `main.ts`). Pour un dépôt public, l'anglais conditionne
l'adoption. L'approche la moins coûteuse : extraire les chaînes dans un module de
traduction avant qu'elles ne se multiplient — le coût croît avec le temps.

### 2.3 Import depuis PuTTY et MobaXterm

Argument d'adoption le plus direct : un utilisateur qui retrouve ses connexions
sans les ressaisir reste. Les formats sont documentés et lisibles.

---

## Axe 3 — Performance, avec des chiffres

Les optimisations faites jusqu'ici l'ont été **sans profilage** : elles reposent
sur des choix raisonnables (regroupement des messages, cadence adaptative), pas
sur des mesures de ce qui coûte réellement.

- **Fait, et payant.** Le gel au redimensionnement a été attribué par `perf` au
  renouvellement des tampons GPU (TTM/GEM), 42 % du coût :
  `WEBKIT_DISABLE_COMPOSITING_MODE` l'a supprimé, confirmé en usage. L'audit a
  ensuite trouvé, par lecture, ce qu'aucun profil local n'aurait montré — le
  téléchargement SFTP sans requête en vol ne se voit qu'en latence réelle, et
  Nagle restait actif sur toutes les sessions (russh ne pose pas `TCP_NODELAY`
  par défaut, contrairement à OpenSSH).
- **Fait, et MESURÉ depuis** : le téléchargement SFTP n'avait aucune requête en
  vol et plafonnait à un bloc par aller-retour. Éprouvé contre un vrai
  `internal-sftp` d'OpenSSH (`examples/sftp_probe.rs`), le passage en bandes
  parallèles donne **6,3 × à 7,1 ×** selon la latence (10 à 60 ms d'aller-retour),
  ~2 × en réseau local, et des octets identiques dans tous les cas. Nagle restait
  par ailleurs actif sur toutes les sessions, et un onglet RDP masqué continuait
  de tirer des trames pleines.
- **Fait depuis (0.4.0)** : la boîte englobante des rectangles sales a laissé
  place à une fusion sélective, mesurée sur le fil — 8,39 Mo → 4,36 Mo pour le
  même parcours, avec davantage de trames livrées.
- **Reste à faire, identifié** : le panneau SFTP **rouvre une session SSH
  complète**, chaîne de rebonds comprise, au lieu d'ouvrir un canal sur celle
  de l'onglet : cinq à sept allers-retours avant le moindre listing.
- **Repères de non-régression.** Les mesures existantes (regroupement, décodage
  UTF-8) affichent des chiffres mais rien ne casse s'ils se dégradent. Un seuil
  d'échec les transformerait en garde-fous.
- **Découpage du paquet front.** 577 Ko en un seul module, surtout xterm.js. Un
  chargement différé du terminal accélérerait le premier affichage — à mesurer
  avant de décider : le gain peut être négligeable pour une application locale.

*Principe* : aucune optimisation n'est retenue sans une mesure avant/après.

---

## Axe 4 — Confiance dans le code

- **Couverture de tests** : mesurée le 02/09/2026 avec `cargo-llvm-cov`
  (`cargo llvm-cov --workspace` à la racine, `cargo llvm-cov` dans
  `rdp-sidecar/`), avant le lot de tests qui a suivi. Cœur et interface : 71 %
  des lignes ; processus RDP : 66 %. Le cœur SSH est entre 82 % et 96 % par
  fichier ; les trous sont l'interface Tauri (`commands.rs` 39 %, `rdp.rs`
  22 % : des commandes qui exigent un état Tauri, désormais construit par le
  moteur d'exécution factice), l'outil en ligne de commande (0 %, désormais
  exercé comme binaire), et dans le processus RDP la boucle de connexion
  (`main.rs` 47 %), que seuls la conformité et les scénarios bout en bout
  traversent. Prochaine étape : remesurer après ce lot et suivre la courbe.
- **Contrôle des dépendances** : `cargo-audit` signale les vulnérabilités connues,
  mais rien ne surveille les licences ni les dépendances abandonnées. `cargo-deny`
  couvre les deux, et sert la conformité AGPL.
- **Test par fuzzing** du parseur `~/.ssh/config` : il lit un fichier que
  l'utilisateur peut avoir édité à la main, c'est la surface d'entrée la plus
  exposée du cœur.
- **Accessibilité au clavier** : les boîtes de dialogue sont traitées ; la liste
  d'hôtes ne se parcourt pas encore aux flèches.

---

## Axe 5 — Fonctionnalités

Par ordre de valeur décroissante :

1. **Enregistrement de session** (format asciinema) — traçabilité, revue d'incident.
2. **Santé des hôtes** — état d'accessibilité visible sans ouvrir de session.
3. **VNC** — complète la couverture des bureaux distants.
4. **Port série** — utile en environnement réseau et industriel.
5. **Transfert de fichiers par RDP** — le presse-papiers texte fonctionne, les
   fichiers restent à faire.

---

## Comment savoir si l'on progresse

Ces mesures sont à relever à chaque version :

| Indicateur | Aujourd'hui | Cap |
|---|---|---|
| Plateformes réellement livrées | 2 | 3 |
| Scénarios bout en bout | 40 | en hausse à chaque fonctionnalité |
| Couverture des tests | 71 % des lignes (cœur + interface), 66 % (processus RDP) | en hausse à chaque version |
| Latence à la frappe (SSH local) | non mesurée | mesurée, < 16 ms |
| Régressions arrivées à l'utilisateur | — | zéro |

Le dernier indicateur est le seul qui compte vraiment. Les autres servent à le tenir.

---

## La leçon du 31 août : trois défauts, zéro test capable de les voir

En une soirée, trois défauts RDP ont été corrigés — image cisaillée sur xrdp,
clavier interprété en QWERTY, connexion suspendue sans fin. **Tous les trois ont
été signalés par Adrien en utilisant avash. Aucun n'était visible depuis les
tests**, dont la suite était pourtant à 295 cas et entièrement verte.

Ce n'était pas un manque de tests, mais un manque de **niveau** de test :

| Ce qu'on avait | Ce que cela voit | Ce que cela ne voit pas |
|---|---|---|
| tests unitaires | nos fonctions, isolément | ce que le serveur envoie vraiment |
| tests d'intégration SSH | un vrai serveur SSH | rien du côté RDP |
| suite bout en bout | l'interface, ses onglets, ses modales | le contenu de l'image RDP |

Entre les deux vivait le seul endroit où ces défauts logeaient : **le dialogue
réel avec un serveur RDP**. Trois conséquences pratiques :

1. **Un parc de vrais serveurs xrdp** (`tests-parc/`), en conteneur, avec deux
   bureaux — XFCE et GNOME — parce que la diversité de rendu est ce qui fait
   sortir les défauts de décodage.
2. **Regarder l'image, pas seulement le code de retour.** Une image cisaillée
   reste plausible : ni somme de contrôle, ni moyenne de pixels ne l'auraient
   vue. Il a fallu un détecteur qui cherche la bonne propriété.
3. **Éprouver l'oracle lui-même.** Le détecteur n'a de valeur que parce qu'on a
   vérifié qu'il distingue franchement une image cisaillée d'une image saine, en
   désactivant le correctif exprès. Un test qui ne peut pas échouer ne protège
   rien — cela vaut aussi pour l'instrument de mesure.

Et une constatation d'ordre différent : **GitLab ne vérifiait rien**. Le dépôt y
était poussé à chaque fois, mais seule la chaîne GitHub contrôlait quoi que ce
soit. Un miroir sans garde-fou est un endroit où une régression peut dormir.

## Franchi : le pipeline graphique (EGFX) — version 0.5.0

GNOME Remote Desktop — et tout serveur qui suit le même chemin — n'envoie ses
images que par le canal dynamique `Microsoft::Windows::RDS::Graphics`. IronRDP
fournissait les **codecs** (`zgfx`, `progressive`, `clearcodec`) mais pas la
couche protocole ; elle est écrite (`egfx.rs`, `progressif.rs`). Les quatre
étapes prévues sont faites : capacités échangées, redirection suivie avec
RDSTLS, surfaces gérées, trames accusées. SLED 16 s'affiche et se redimensionne.

Ce que cette étape a appris, et qui vaut au-delà d'elle :

- **Un message bien formé peut parler d'autre chose.** Notre `CAPS_ADVERTISE`
  portait l'identifiant `0x0011` — `CacheImportReply` — au lieu de `0x0012`. Le
  serveur le lisait sans erreur, puis fermait la session dix secondes plus tard
  sur un `BadCapabilities` qui désignait la bonne famille de problème et la
  mauvaise cause. Aucune relecture du code ne l'aurait montré : le code était
  cohérent avec lui-même.
- **Quand un serveur reste muet, mettre à côté un client qui, lui, obtient une
  réponse.** Remmina fonctionnait ; Remmina emploie FreeRDP ; FreeRDP est
  installable ici. Capturer sa session, la déchiffrer, comparer les octets aux
  nôtres : le défaut est sorti en une lecture.
- **Un délai de garde n'est pas un refus.** Les dix secondes entre notre envoi
  et l'erreur disaient que le serveur *attendait* quelque chose, non qu'il
  rejetait ce qu'on lui avait donné. Cette distinction a redirigé toute
  l'enquête.

Reste ouvert, faute de matériel : **ce qu'un serveur Windows enverrait sur ce
canal**. Aucun n'existe dans le parc, et les commandes qu'il emploie
(`SOLIDFILL`, `SURFACE_TO_SURFACE`, les caches, H.264) ne sont pas décodées.
C'est pourquoi le canal est refusé par défaut et n'est accordé qu'aux serveurs
qui ont montré n'avoir que celui-là : ne pas savoir doit se traduire par ne pas
proposer. La leçon a été chère — la première version de ce garde-fou retenait
seulement l'annonce de capacités, et deux Windows du parc réel sont passés à
l'écran noir. Accepter le canal suffit à faire taire un serveur, avant même
qu'on lui ait dit quoi que ce soit.

## Règles de travail

Apprises en construisant le projet, coûteuses à réapprendre :

- **Ne jamais annuler un travail non commité avec `git checkout`.** Fait deux
  fois, et rattrapé la seconde uniquement grâce à une sauvegarde faite quelques
  minutes plus tôt. Pour défaire une modification temporaire (un contrôle
  négatif, par exemple) : copier le fichier avant, restaurer la copie après.
- **Vérifier un binaire compilé sur ses littéraux, jamais sur ses noms de
  fonctions.** Le profil release pose `strip = true` : les symboles n'y sont
  plus. Un contrôle cherchant des noms de fonctions a annoncé « correctifs
  absents » alors qu'ils étaient bien là.

- **Le binaire embarque le front.** Après toute modification de `web/`, faire
  `vite build` *puis* `cargo build --release` — sinon l'application testée conserve
  l'ancienne interface.
- **Reconstruire la distribution locale à chaque version.** `./check.sh --quick`
  saute délibérément le build release : le binaire de `target/release/` et
  l'AppImage de `dist-release/` restent alors ceux d'avant les correctifs. Après
  toute correction destinée à l'utilisateur, lancer `./scripts/release.sh`
  (`NO_STRIP=1` sur Arch) — sinon il essaie la version publiée en ligne pendant
  que sa copie locale est périmée, et les deux ne se comportent pas pareil.
- **`confirm()` et `prompt()` sont inopérants sous WebKitGTK.** Utiliser
  `askConfirm()` / `askText()`. La garde `scripts/guard.sh` interdit leur retour.
- **Un nouveau test doit être vu échouer.** Débrancher la fonctionnalité qu'il
  couvre et vérifier qu'il tombe : un test qui ne peut pas échouer ne protège rien.
- **Un serveur de test par scénario.** Un serveur partagé entre fichiers de tests
  rend la suite instable.
- **Attendre un état, jamais une durée.** Les échecs intermittents viennent presque
  tous d'une interrogation faite trop tôt. Et « le port répond » n'est pas un
  état suffisant : un serveur qui traite ses clients l'un après l'autre doit être
  vu *revenir* accepter.
- **Une référence d'élément devient caduque dès que la liste est reconstruite.**
  Parcourir `$$()` puis interroger chaque ligne lève une erreur si un rendu
  s'est glissé entre les deux — et à l'intérieur d'un `waitUntil`, cette erreur
  **avorte l'attente** au lieu de la faire réessayer. C'était la cause d'une
  famille entière d'échecs intermittents, invisibles sur une machine au repos.
  Une recherche doit rendre « pas trouvé », jamais lever.
- **Un instrument de mesure se vérifie avant la mesure.** Un premier relais
  destiné à simuler de la latence lisait un bloc, dormait, puis écrivait — ce
  qui bloque la lecture pendant l'attente et sérialise justement le pipeline
  qu'on veut mesurer. Il donnait 3 × là où un modèle correct de délai de
  propagation donne 7 ×. Une mesure qui contredit la théorie accuse d'abord
  l'instrument.
- **Éprouver la stabilité sous charge, pas par répétition.** Relancer la suite
  dix fois sur une machine oisive ne révèle rien ; la lancer une fois pendant
  trois compilations concurrentes révèle tout. C'est la fenêtre temporelle qu'on
  cherche à élargir, pas le nombre de tirages.
- **Un serveur de test complaisant ne prouve rien.** Le nôtre rendait le même
  bloc quel que soit le décalage demandé : il aurait validé n'importe quel
  lecteur, y compris un lecteur en bandes parallèles qui réassemble de travers.
  Un mock doit être aussi exigeant que la chose qu'il remplace.
- **Ne jamais annuler un contrôle négatif par `git checkout`.** Il emporte tout
  le travail non commité du fichier. Défaire exactement l'édition faite.
- **Juger une exécution de tests à son code de sortie, pas à un `grep`.** Un
  débordement de pile écrit « fatal runtime error », pas « FAILED » ni
  « panicked » : trois exécutions ont été déclarées vertes alors qu'elles
  échouaient, parce que le motif cherché ne couvrait pas ce cas. `check.sh`, lui,
  regarde le code de sortie — c'est lui qui a vu juste.
- **Un remplacement en masse relit ce qu'il vient d'écrire.** Remplacer
  `dirs::home_dir()` par `repertoire_personnel()` dans tout un fichier a
  transformé le repli de cette fonction en appel à elle-même. Après un
  `sed`/`replace` global, relire la définition de ce qu'on a introduit.

---

## Hors périmètre

Décisions prises, à ne pas rouvrir sans raison nouvelle :

- **Pas d'Electron.** Le poids et la mémoire sont des arguments de vente.
- **Pas de télémétrie**, même anonyme. C'est un outil d'administration.
- **Pas de synchronisation par un service tiers.** `~/.ssh/config` est déjà
  versionnable ; y ajouter un compte en ligne ajouterait une surface d'attaque.
- **TypeScript 7** tant que `typescript-eslint` ne le prend pas en charge : il
  désactiverait le lint typé, qui a déjà attrapé des bugs réels.

[suse2]: https://www.suse.com/c/headless-remote-sessions-in-gnome-part-2/
