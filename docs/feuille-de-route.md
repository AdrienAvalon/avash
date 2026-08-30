# Feuille de route

Ce document fixe le cap d'avash et sert de point de reprise entre les sessions de
travail. Il est volontairement **fondé sur des constats mesurés**, pas sur des
intentions : chaque objectif est vérifiable.

Dernière révision : 30 août 2026 (version 0.2.0).

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

| Indicateur | Valeur au 30/08/2026 |
|---|---|
| Tests | 139 Rust · 61 front · 24 bout en bout |
| Binaire Linux | 18,3 Mo (`codegen-units=1`, LTO fin) |
| Paquet front | 577 Ko en un seul module |
| Plateformes construites | Linux (AppImage) et Windows (NSIS) — Windows pas encore éprouvé sur machine réelle |
| Dette déclarée | aucun `TODO`/`FIXME` dans le code |
| Licence | AGPL-3.0-or-later (+ licence commerciale possible) |

Acquis récents : presse-papiers RDP testé de bout en bout, accessibilité des
boîtes de dialogue, cadence adaptative RDP, arborescence de dossiers.

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

### 1.2 Construire réellement pour Windows — **construit, pas encore éprouvé**

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

**Reste à faire** : lancer l'installeur sur une vraie machine Windows. Les points
à risque sont le sidecar RDP, le trousseau (Credential Manager) et les chemins de
configuration. Tant que ce n'est pas fait, Windows est *compilé*, pas *validé*.

*Fini quand* : un installeur Windows se lance, ouvre une session SSH et un bureau RDP.

### 1.3 Isoler l'état entre les fichiers de test — **fait**

Les scénarios partageaient un seul bac à sable : un fichier qui créait un dossier
ou déplaçait un hôte faussait les suivants. Corrigé — le bac à sable est remis à
son état semé avant chaque fichier, et un garde-fou (`isolation.spec.js`) le
vérifie. Il a été vu échouer avant correction, conformément à la règle.

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

- **Mesurer avant d'optimiser.** Un profil à la flamme (`cargo flamegraph`) sur une
  session SSH chargée et sur un flux RDP dira où passe le temps. Sans cela, toute
  optimisation supplémentaire est une supposition.
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
| Plateformes réellement livrées | 1 | 3 |
| Scénarios bout en bout | 24 | en hausse à chaque fonctionnalité |
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
  tous d'une interrogation faite trop tôt.

---

## Hors périmètre

Décisions prises, à ne pas rouvrir sans raison nouvelle :

- **Pas d'Electron.** Le poids et la mémoire sont des arguments de vente.
- **Pas de télémétrie**, même anonyme. C'est un outil d'administration.
- **Pas de synchronisation par un service tiers.** `~/.ssh/config` est déjà
  versionnable ; y ajouter un compte en ligne ajouterait une surface d'attaque.
- **TypeScript 7** tant que `typescript-eslint` ne le prend pas en charge : il
  désactiverait le lint typé, qui a déjà attrapé des bugs réels.
