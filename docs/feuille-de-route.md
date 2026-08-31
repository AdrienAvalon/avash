# Feuille de route

Ce document fixe le cap d'avash et sert de point de reprise entre les sessions de
travail. Il est volontairement **fondé sur des constats mesurés**, pas sur des
intentions : chaque objectif est vérifiable.

Dernière révision : 31 août 2026, après un multi-audit complet (fiabilité,
sécurité, performance, ergonomie) et six lots de corrections.

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

| Indicateur | Valeur au 31/08/2026 |
|---|---|
| Tests | 160 Rust (106 cœur, 20 intégration, 34 interface) · 78 front · 18 fichiers bout en bout |
| Binaire Linux | 18 Mo (`codegen-units=1`, LTO fin) |
| Paquet front | 572 Ko en un seul module |
| Plateformes livrées | Linux (AppImage) et Windows (NSIS + portable), éprouvées sur machine réelle |
| Dette déclarée | aucun `TODO`/`FIXME` dans le code |
| Licence | AGPL-3.0-or-later (+ licence commerciale possible) |

Acquis récents : Windows validé en usage réel (RDP, clavier, mise à jour
automatique), gel au redimensionnement corrigé après profilage, et un
multi-audit dont les constats sont traités ci-dessous.

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

## Le multi-audit du 31 août — ce qu'il a trouvé

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

Ce que l'exercice enseigne : les défauts les plus graves n'étaient pas dans le
code neuf, mais dans les hypothèses tacites — « la bibliothèque vérifie », « ce
chemin n'arrive jamais », « le serveur est honnête ».

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
- **Reste à mesurer** : un profil à la flamme sur un flux RDP chargé. Le sidecar
  fusionne encore ses rectangles sales en boîte englobante — deux coins opposés
  produisent une trame plein écran, 8 Mo là où 2 Ko suffiraient.
- **Repères de non-régression.** Les mesures existantes (regroupement, décodage
  UTF-8) affichent des chiffres mais rien ne casse s'ils se dégradent. Un seuil
  d'échec les transformerait en garde-fous.
- **Découpage du paquet front.** 577 Ko en un seul module, surtout xterm.js. Un
  chargement différé du terminal accélérerait le premier affichage — à mesurer
  avant de décider : le gain peut être négligeable pour une application locale.

*Principe* : aucune optimisation n'est retenue sans une mesure avant/après.

---

## Axe 4 — Confiance dans le code

- **Couverture de tests** : aucun outil de mesure installé. `cargo-llvm-cov`
  révélerait les chemins jamais exercés — utile surtout sur le code de sécurité.
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
| Scénarios bout en bout | 18 fichiers | en hausse à chaque fonctionnalité |
| Couverture des tests | non mesurée | mesurée, puis en hausse |
| Latence à la frappe (SSH local) | non mesurée | mesurée, < 16 ms |
| Régressions arrivées à l'utilisateur | — | zéro |

Le dernier indicateur est le seul qui compte vraiment. Les autres servent à le tenir.

---

## Règles de travail

Apprises en construisant le projet, coûteuses à réapprendre :

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
- **Un serveur de test complaisant ne prouve rien.** Le nôtre rendait le même
  bloc quel que soit le décalage demandé : il aurait validé n'importe quel
  lecteur, y compris un lecteur en bandes parallèles qui réassemble de travers.
  Un mock doit être aussi exigeant que la chose qu'il remplace.
- **Ne jamais annuler un contrôle négatif par `git checkout`.** Il emporte tout
  le travail non commité du fichier. Défaire exactement l'édition faite.

---

## Hors périmètre

Décisions prises, à ne pas rouvrir sans raison nouvelle :

- **Pas d'Electron.** Le poids et la mémoire sont des arguments de vente.
- **Pas de télémétrie**, même anonyme. C'est un outil d'administration.
- **Pas de synchronisation par un service tiers.** `~/.ssh/config` est déjà
  versionnable ; y ajouter un compte en ligne ajouterait une surface d'attaque.
- **TypeScript 7** tant que `typescript-eslint` ne le prend pas en charge : il
  désactiverait le lint typé, qui a déjà attrapé des bugs réels.
