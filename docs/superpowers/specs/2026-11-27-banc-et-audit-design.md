# Banc de mesures et audit complet : conception

Rédigé le 27 novembre 2026, à la demande du mainteneur, après la publication de
la 0.16.0.

## Intention

Le mainteneur se sert d'Avash **intensément, sous Windows, au travail** :
plusieurs SSH, RDP vers des Windows, xrdp, SFTP, **environ 10 sessions ouvertes
en même temps**. Il ne remarque pas de lenteur particulière, mais veut
« améliorer au maximum » Avash : audit complet, tests renforcés, optimisation et
performances, sous Windows comme sous Linux. « On y va à fond. »

Puisque rien ne se voit à l'œil nu, les gains doivent se mesurer. Il faut donc
d'abord un instrument qui donne des chiffres reproductibles et comparables d'une
version à l'autre, avant tout audit et toute optimisation (approche A,
ci-dessous).

## Objectifs

1. **Un banc unique** qui rejoue l'usage réel : 10 sessions mixtes (SSH, RDP
   vers Windows, xrdp, SFTP) au même moment. Il pousse aussi au-delà (20, 30)
   pour voir où ça cède. Il tourne sous Linux et sous Windows, et sort ses
   chiffres dans un format commun.
2. **La comparaison de deux versions** (référence contre candidate) sur la même
   machine, avec un verdict qui tient compte du bruit.
3. **Une garde en CI** : un sous-ensemble rapide du banc, en A/B sur le même
   exécuteur, qui bloque une régression nette. Le banc complet tourne chaque
   nuit sur `main`.
4. **Une ligne de base 0.16.0** sous Linux et sous Windows, consignée dans
   `docs/feuille-de-route.md` (axe 3).
5. **Un audit complet** par une équipe d'agents (accord du mainteneur le
   27/11/2026). Il couvre la performance, la mémoire, la fiabilité sous 10
   sessions, ce qui est propre à Windows, la qualité des tests et la sécurité.
   Chaque constat est vérifié, et chaque optimisation est chiffrée avant et
   après par le banc.
6. **Des tests renforcés** : trous de couverture comblés, tests instables
   repérés et corrigés, scénarios fonctionnels à 10 sessions.
7. **Une 0.17.0** qui livre le tout.

## Non-objectifs

- **FTP et FTPS.** Avash ne les gère pas, et le mainteneur s'en sert au travail
  avec un autre outil. Ce serait une nouvelle fonctionnalité, à traiter à part
  si le mainteneur le souhaite.
- **VNC.** Le mainteneur ne le cite pas : il garde ses tests actuels, mais n'a
  pas de scénario de banc dédié.
- **Le « mode mesure » intégré à Avash** (approche C). Ce relevé, exporté
  depuis le PC du travail, est écarté de ce chantier. On y reviendra si le PC
  du travail ne peut rien lancer.
- **macOS comme plateforme de mesure.** La CI macOS garde ses tests.

## Anti-objectifs (ce qui serait un échec même si « ça marche »)

- **Un gain de vitesse qui affaiblit la sécurité.** La sécurité prime dans tout
  arbitrage. Si un compromis s'impose, il est soumis au mainteneur.
- **Une optimisation visible** : rendu dégradé, couleurs, latence perçue.
- **Un banc qui fait rougir la CI au hasard.** Une garde instable sera vite
  ignorée, ce qui est pire que pas de garde.
- **Des chiffres annoncés sans mesure reproductible derrière.** Ce fut le cas du
  « 409 ms » du 26/11, faussé par le banc lui-même.
- **Un binaire publié qui contient la fonctionnalité cargo `webdriver`.**

## Contraintes

- **Les règles du dépôt** (`CLAUDE.md`, `CONTRIBUTING.md`) s'appliquent :
  - `./check.sh` avant chaque commit, sans toucher l'arbre pendant qu'il tourne ;
  - PR, puis CI vérifiée ligne à ligne avant fusion ;
  - pousser sur les deux distants ;
  - aucun secret dans un fichier suivi ;
  - pas de `cargo fmt` sur `rdp-sidecar/vendor/` (rustfmt fichier par fichier,
    `max_width=120`).
- **Le serveur WebDriver embarqué** (Windows, macOS) ne fait parvenir aucun
  caractère de contrôle au shell et double les caractères de `browser.keys`.
  Les scénarios Windows tapent par `taperDansLeTerminal` et lisent leurs
  résultats dans la sortie du PTY.
- **Les processus à compter.** Sous Windows, la page tourne dans des processus
  `msedgewebview2.exe` distincts, qu'il faut compter dans la mémoire et le
  processeur. Sous Linux, WebKitGTK a ses propres processus (`WebKitWebProcess`,
  `WebKitNetworkProcess`).
- **La machine de mesure** doit être au repos, comme dans
  `scripts/mesures-front.sh`. Sous Linux, le poste partage la RTX 3090 avec le
  bureau et Ava : le banc refuse de tourner sous charge, et ne lance rien de
  lourd sur le GPU.
- **Les mesures se prennent hors bureau Wayland** (`WAYLAND_DISPLAY` retiré) et
  avec un `~/.cache` dans le bac à sable : leçon du 26/11.
- **Le temps de CI.** La garde sur PR doit tenir dans un budget raisonnable
  (cible : moins de 15 minutes de plus par plateforme, hors cache).

## Approches envisagées

- **A. Le banc d'abord, puis l'audit appuyé sur ses chiffres (retenue).**
  Les gains sont prouvés et la base est durable. En contrepartie, les premières
  corrections arrivent plus tard.
- **B. L'audit d'abord** (méthode de septembre), avec des mesures seulement là
  où un constat les demande. On obtient des résultats plus vite, mais Windows
  reste peu mesuré et les gains de vitesse restent souvent des estimations.
- **C. A, plus un « mode mesure » intégré** que le mainteneur exporte depuis son
  PC du travail. On aurait des chiffres du vrai usage, mais au prix d'une
  fonctionnalité de plus. Reportée.

Le mainteneur a laissé le choix de la méthode. A a été retenue, et l'équipe
d'agents confirmée.

## Conception

### 1. Scénarios du banc

Chaque scénario rend des métriques nommées, chacune avec médiane, p95, minimum,
maximum et nombre d'échantillons.

| Id | Scénario | Métriques principales |
|---|---|---|
| S1 | Démarrage à froid et à chaud, jusqu'à la liste interactive | temps, mémoire de base de tous les processus |
| S2 | Latence de frappe SSH, avec 1 puis 10 sessions actives | écho, peinture à l'écran |
| S3 | Débit du terminal (`cat` de 50 Mo, flot continu) | temps d'affichage, processeur, réactivité de l'interface pendant le flot |
| S4 | **10 sessions mixtes** : 6 SSH, 1 RDP vers Windows, 1 xrdp, 1 RDP mire, 1 SFTP en transfert ; puis 20 et 30 | mémoire totale, processeur au repos et en activité, latence de frappe dans l'onglet actif, temps d'ouverture d'un onglet, i/s de l'onglet RDP actif |
| S5 | Bureau à distance : mire RemoteFX 1080p (existant), xrdp réel, RDP vers un vrai Windows | i/s, débit, aller-retour, temps jusqu'à la première image, processeur du processus RDP |
| S6 | SFTP : montée et descente de 100 Mo et de 1 000 petits fichiers, à 0 et 40 ms d'aller-retour | Mo/s, temps total |
| S7 | Connexion : jusqu'à l'invite SSH, jusqu'à la première image RDP | temps |
| S8 | Endurance : 10 sessions pendant 30 minutes (nuit seulement) | pente de la mémoire (fuites), dérive de la latence |

Plus des **micro-bancs Rust**, sur le modèle du test de vitesse RLGR ignoré par
défaut : décodage RLGR et DWT, relais PTY et regroupement, décodage UTF-8,
analyse de la configuration SSH. Ils sortent le même format.

**Serveurs utilisés :**
- sshd local du harnais e2e, déjà en place ;
- `test-rdp-server --mire`, déjà en place ;
- parc xrdp (`scripts/parc-rdp.sh`, Linux seulement) ;
- la **VM Windows 11** comme vrai serveur RDP (127.0.0.1:13389) pour S4, S5 et
  S7 lancés depuis Linux. Sous Windows, le RDP vers Windows vise la machine
  elle-même ou n'est pas mesuré : choix laissé au constructeur, à documenter.
- La latence injectée (S6 à 40 ms) passe par `netem` sous Linux. Sous Windows,
  S6 ne se mesure qu'en local.

### 2. Format et comparaison

- **Un JSON par exécution**, contenant :
  - version, commit, plateforme, description de la machine (processeur,
    mémoire, charge au départ), date ;
  - pour chaque scénario, ses métriques.
- Les résultats restent hors du dépôt (`.gitignore`). La ligne de base de
  chaque version publiée est consignée en Markdown dans `docs/` ; le JSON ne
  l'est pas.
- **`comparer` (deux JSON vers un tableau Markdown)** :
  - il donne l'écart relatif de chaque métrique et classe chacune en mieux,
    pire ou dans le bruit ;
  - le bruit est mesuré par les répétitions (au moins 5 par métrique) ;
  - un écart sous la dispersion n'est pas un verdict.

### 3. Exécution A/B

- **Un point d'entrée unique** pour Linux et Windows : `scripts/banc.sh` et
  `scripts/banc.ps1`, ou un seul script Node du harnais. Choix laissé au
  constructeur.
- Il **construit deux références** (par exemple `v0.16.0` et `HEAD`) dans des
  `git worktree` séparés, **avec le profil release publié** (pas de profil
  allégé : on mesure ce qui est livré). La fonctionnalité `webdriver` n'est
  posée que là où le harnais l'exige (Windows) ; sous Linux, tauri-driver
  pilote le binaire sans elle. Il pilote les
  deux binaires **en alternance** sur la même machine (ABAB…), avec le harnais
  de `HEAD`. L'alternance neutralise la dérive thermique et la charge de fond.
- **La compatibilité du harnais** n'est garantie qu'à partir de la 0.16.0, qui
  sert de ligne de base. Les versions plus anciennes ne sont pas prises en
  charge.
- Les binaires de banc ne quittent jamais la machine où ils sont construits :
  ni artefact publié, ni envoi au PC du travail.

### 4. CI

- **Sur chaque PR**, un job « Banc (A/B) » par plateforme (Linux, Windows). Il
  fait tourner un sous-ensemble rapide (S1, S2 à 10 sessions, S5 mire, S4 à 10
  sessions SSH et mire, sans VM ni xrdp) : base de la PR contre tête, sur le
  même exécuteur.
  - Il **échoue** sur une régression au-delà d'un seuil large **et** hors du
    bruit.
  - Il publie le tableau de comparaison dans le résumé du job.
  - Les seuils de départ sont réglés pour ne pas rougir sur 10 exécutions à
    vide (base contre elle-même), et consignés.
- **Chaque nuit** sur `main` : banc complet sous Linux (S1 à S8 sauf VM et
  xrdp, que les exécuteurs hébergés n'ont pas ; xrdp déjà présent dans le job
  « Conformité » peut servir) et sous Windows. Le résultat est versé en artefact.

### 5. Audit

Il démarre une fois la ligne de base 0.16.0 posée. Il est mené par une équipe
d'agents, comme le 12 septembre, chaque dimension recevant les chiffres du banc
et le profil (`perf` sous Linux, ETW ou WPR sous Windows si disponible dans la
VM) :

1. **Performance Rust** (cœur, interface, processus RDP) ;
2. **Performance du front** (rendu, xterm.js, DOM, 10 onglets) ;
3. **Mémoire et fuites** (S8, onglets ouverts puis fermés) ;
4. **Spécificités Windows** :
   - WebView2 ;
   - trousseau ;
   - chemins ;
   - pare-feu ;
   - DPI ;
   - comportement à 10 sessions ;
5. **Fiabilité à 10 sessions et plus** :
   - concurrence ;
   - reconnexion ;
   - coupure réseau ;
   - mise en veille ;
6. **Qualité des tests** :
   - couverture réelle (`scripts/couverture.sh`) ;
   - tests instables (relances répétées de la suite e2e) ;
   - trous (RDP réel, SFTP sous Windows) ;
   - mutation testing : cargo-mutants tourne déjà dans le workflow qualité
     sur les modules de sécurité ; l'étendre aux modules que l'audit juge
     faibles ;
7. **Sécurité** : nouvelle passe complète.

Chaque constat est **vérifié par un second agent** (rejeté s'il ne se reproduit
pas). Les corrections se font par **voies** qui se partagent les fichiers (même
découpage qu'en septembre : CŒUR, INTERFACE, SIDECAR, FRONT, CHAÎNE), en PR
séparées. Chaque PR de performance porte son tableau avant/après sorti du banc.
Une optimisation qui ne gagne rien de mesurable n'est pas fusionnée.

Les rapports sont copiés dans `~/dev/audits/avash-2026-11-27/`.

### 6. Livraison

1. **Phase 1, le banc** : scénarios, format, `comparer`, A/B, job CI et nuit ;
   ligne de base 0.16.0 sous Linux et sous Windows (VM et CI). Publication en
   une ou plusieurs PR.
2. **Phase 2, l'audit** : constats vérifiés, rapport au mainteneur avec ses
   priorités.
3. **Phase 3, les corrections** par vagues, chacune mesurée.
4. **Phase 4, la 0.17.0** et les canaux.

Le mainteneur reçoit un point d'étape à la fin de chaque phase.

## Ce qui reste au constructeur

- Le langage et la forme du point d'entrée du banc (Bash et PowerShell, ou
  Node).
- La façon de produire les 10 sessions : par l'interface pilotée (réaliste) ou
  en partie par des commandes internes (plus rapide). Au moins S2 et S4 passent
  par l'interface.
- La façon de mesurer la mémoire sous Windows (working set privé de chaque
  processus, WebView2 compris) et sous Linux (PSS ou RSS, à justifier).
- Les valeurs exactes des seuils de CI, d'après les exécutions à vide.
- L'outil de micro-banc. Préférence : pas de nouvelle dépendance si le modèle
  des tests ignorés suffit. Une dépendance nouvelle passe `cargo deny`.
- L'ordre interne des scénarios et leur durée, dans le budget de CI.

## Décisions de mise en œuvre

Ces choix ont été arrêtés après la relecture du document par un constructeur.

- **Les phases s'enchaînent sans attendre.** Le mainteneur, souvent à
  distance, reçoit un point d'étape à la fin de chaque phase, mais le travail
  ne s'y arrête pas : tout constat vérifié est corrigé. Deux exceptions
  arrêtent la voie concernée jusqu'à sa réponse : un compromis de sécurité, et
  un changement visible du produit (comportement, réglage, interface).
- **Le budget de CI** s'entend avec un cache Rust chaud. Pour tenir avec deux
  compilations release par plateforme, le sous-ensemble de PR est court. Si
  Windows ne tient pas dans le budget, son job tourne sur les PR qui touchent
  du code (pas la documentation seule).
- **La nuit** fait tourner, sous Linux et sous Windows :
  - un A/B du sous-ensemble de PR, entre la dernière version publiée et
    `main` ;
  - S8 en une seule exécution de `main`, jugée sur un seuil absolu de pente
    mémoire.
  Le job échoue sur une régression ou une fuite, et GitHub prévient le
  mainteneur par sa notification d'échec ordinaire. Pas d'ouverture
  automatique de ticket.
- **Sous Windows, la ligne de base se prend dans la VM**, avec des binaires
  construits dans la VM elle-même. On y installe la chaîne (Build Tools, Rust,
  Node, OpenSSH Server) par les scripts de `~/VMs/scripts` déjà écrits pour la
  machine précédente. Les binaires `webdriver` ne quittent donc jamais la
  machine qui les construit. Les exécuteurs de CI donnent le second point de
  vue sous Windows.
- **Le jeu de règles de `main` ne change pas.** Le job « Banc (A/B) » bloque
  par la règle déjà suivie : aucune fusion tant qu'une ligne de
  `gh pr checks` n'est pas verte. Si le bruit des exécuteurs Windows impose
  un seuil qui ne détecte plus rien, le job Windows devient informatif (il
  publie le tableau sans échouer), et c'est écrit dans `docs/qualite.md`. Le
  job Linux reste bloquant.

## Points tranchés par l'assistant (à vérifier par le mainteneur)

- Windows est mesuré sur la VM et sur les exécuteurs de CI, jamais sur le PC du
  travail pendant ce chantier.
- La 0.16.0 sert de ligne de base. Pas de comparaison avec les versions plus
  anciennes.
- VNC et FTP sont hors périmètre.
- Les phases s'enchaînent sans attendre la validation du mainteneur, sauf
  compromis de sécurité ou changement visible du produit.
- Le banc mesure le profil release publié.
