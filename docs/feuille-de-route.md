# Feuille de route

Ce document fixe le cap d'avash et sert de point de reprise entre les sessions de
travail. Il est volontairement **fondé sur des constats mesurés**, pas sur des
intentions : chaque objectif est vérifiable.

Dernière révision : 5 septembre 2026, après la publication de la version 0.8.0
— un lot de fond (panneau SFTP sur la session du terminal, mot de passe oublié
dès la connexion, front et processus RDP découpés en modules, chaîne
d'intégration complète, quarante tests de plus, fuzzing du parseur), après la
0.6.1 qui durcissait le collage, la webview et les traces RDP.

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

| Indicateur | Valeur au 05/09/2026 |
|---|---|
| Tests | 440 Rust (158 cœur, 47 intégration, 72 interface, 134 processus RDP, 29 serveurs de test) · 595 dans les paquets IronRDP et vnc-rs portés · 115 front · 69 scénarios bout en bout dans 35 fichiers, tous en intégration continue, sous Linux et sous Windows (serveurs locaux compris depuis le 05/09/2026), et hors serveurs locaux sous macOS |
| Binaire Linux | 18 Mo (`codegen-units=1`, LTO fin) ; AppImage publiée 85 Mo |
| Paquet front | 172 Ko de paquet principal ; xterm.js (331 Ko) et ses extensions (WebGL 113, recherche 32, sérialisation 15, liens 2, ajustement 1) chargés à part, à l'oisiveté après l'accueil |
| Plateformes livrées | Linux (AppImage) et Windows (NSIS + portable), éprouvées sur machine réelle ; macOS (image disque) construite et testée en CI, pas encore éprouvée |
| Dette déclarée | aucun `TODO`/`FIXME` dans le code |
| Version publiée | 0.8.0 (Linux AppImage, deb et rpm, Windows, macOS ; signées, attestation Sigstore et SBOM) |
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

### 1.5 Avash dans Avash sous Windows — **fait** (04/09/2026)

Un avash Windows lancé dans une session RDP affichait son propre bureau
distant avec des zones noires, des carrés gris, des blocs flous, puis des
icônes noires. Quatre causes, trouvées l'une après l'autre en enregistrant la
session depuis le contrôleur de domaine du parc et en rejouant sans réseau :
le canvas GPU de WebView2 perdait sa texture (canvas logiciel), le codec
progressif ignorait les tuiles en différence et n'affinait rien par paliers
(réécrit d'après FreeRDP), le sous-codec NSCodec de ClearCodec était un bras
vide dans IronRDP (porté, avec le RLEX à une couleur), et deux chemins de
redimensionnement pouvaient vider l'image l'un après l'autre. Deux
enregistrements de référence de plus, un quatrième paquet porté, et la
méthode consignée dans `CONTRIBUTING.md` : mesurer les pixels, comparer à
FreeRDP. Vérifié sur le parc, avash dans avash, après ouverture,
maximisation et restauration.

### 1.4 Le panneau SFTP sur la session du terminal — **fait**

Chaque onglet qui dépliait son panneau de fichiers rejouait la connexion
complète — rebonds, vérification de clé d'hôte, authentification — au lieu
d'ouvrir un canal `sftp` sur la session du terminal, que le protocole permet.
Cela doublait les ressources côté serveur, déclenchait une seconde
authentification, visible dans les journaux et parfois refusée par les
politiques qui bornent les sessions par utilisateur, et obligeait à garder le
mot de passe en mémoire pour le rejeu. Le panneau ouvre désormais son canal
sur la session vivante, partagée avec le relais du terminal ; la cible, mot
de passe compris, n'est plus conservée une fois connecté. Un test
d'intégration compte les connexions vues par le serveur SSH de test : une
seule pour un terminal et son panneau, et le terminal répond toujours pendant
que le panneau liste.

## Axe 2 — Élargir la portée

### 2.1 macOS — **construit, pas encore éprouvé**

Ni construit ni testé jusqu'au 02/09/2026. Désormais : un job `macos-latest`
de la chaîne compile le front, teste le cœur (serveur SSH en mémoire compris),
compile et teste le processus RDP et l'interface à chaque commit ; la release
produit l'image disque `Avash_x.y.z_aarch64.dmg` et l'archive signée que la
mise à jour automatique télécharge, avec l'entrée `darwin-aarch64` du
manifeste. Le trousseau natif Apple est celui de `keyring`. Depuis le soir du
02/09/2026, le même job joue aussi la suite bout en bout sur WKWebView, par le
serveur WebDriver embarqué (macOS n'a aucun pilote) : l'interface y démarre,
s'y pilote, et ses scénarios sans serveur local y passent. Ce qui manque :
une machine pour l'éprouver en usage réel (connexions SSH et RDP effectives),
et une notarisation (identité de développeur Apple) pour épargner le clic
droit → Ouvrir du premier lancement.

### 2.2 Anglais — **fait**

L'interface était entièrement en français, chaînes écrites en dur. Elles sont
extraites dans `web/i18n.ts` : le français reste la source, l'anglais couvre
chaque clé (un test le vérifie, avec les variables et l'application à la page),
les textes statiques portent `data-i18n`, ceux du code passent par `t()`. La
bascule se fait dans la palette et se mémorise ; un scénario bout en bout la
joue dans les deux sens. Au premier lancement, la langue suit la locale du
système : français pour `fr*`, anglais pour le reste ; les tailles (Ko ou KB)
et les dates (jj/mm/aa ou ISO) suivent.

### 2.3 Import depuis PuTTY et MobaXterm — **fait**

Argument d'adoption le plus direct : un utilisateur qui retrouve ses connexions
sans les ressaisir reste. Les sessions PuTTY sont lues dans `~/.putty/sessions`
(fichiers `clé=valeur`, noms encodés en `%XX`) ou dans le registre Windows par
`reg query` ; celles de MobaXterm dans `MobaXterm.ini` ou un export
`.mxtsessions`, avec leur dossier. Seules les sessions SSH sont reprises ; les
autres protocoles sont comptés et dits. Une clé `.ppk` ou un mandataire ne
sont pas repris, et le candidat le signale. L'interface propose la liste, alias
modifiables, un hôte déjà déclaré pour le même serveur décoché d'office, et
écrit dans `~/.ssh/config`. Testé par parseurs (échantillons réels des deux
formats), par les commandes, et de bout en bout sur des sessions PuTTY semées.

### 2.4 Canaux de distribution — **en cours** (04/09/2026)

Un utilisateur installe par son gestionnaire de paquets ou pas du tout. Ce
qui est en place, et ce qui attend :

- **Release GitHub** : AppImage, `.deb` (Debian 12, Ubuntu 22.04 et
  suivants), `.rpm` (Fedora, openSUSE), installeur et archive portable
  Windows, image disque macOS ; empreintes, attestation de provenance
  Sigstore, SBOM SPDX attesté. Les paquets deb et rpm ont été installés et
  lancés dans des conteneurs Ubuntu 24.04 et Fedora avant d'être publiés.
- **winget** (`AdrienCros.Avash`) : première soumission ouverte le
  04/09/2026, validée par le robot, CLA signé ; attend un modérateur. À
  chaque version suivante, `scripts/winget-manifeste.sh`.
- **AUR** : `packaging/aur/avash/PKGBUILD`, éprouvé par `makepkg` sur le
  poste ; la publication attend un compte AUR avec sa clé SSH.
- **Homebrew** : cask `packaging/homebrew/avash.rb`, à soumettre depuis un
  Mac (`brew bump-cask-pr`), après un premier essai réel de l'image disque.
- **Flathub** : manifeste `packaging/flathub/io.github.AdrienAvalon.avash.yml`,
  construction hors ligne (GNOME 49, Rust stable et Node 22 du SDK, sources
  cargo et npm figées), construit, installé et lancé sur le poste par
  `flatpak-builder`, depuis le tag v0.8.0 ; le linter Flathub passe hormis
  trois droits qui demandent une exception justifiée (agent SSH, dossier
  personnel, nom D-Bus de Tauri). La branche de soumission est prête sur le
  fork ; la PR elle-même revient au mainteneur en personne : la politique de
  Flathub sur l'IA générative exige de déclarer le code écrit avec Claude et
  interdit qu'un agent ouvre la PR (la #10077, ouverte sans la liste à cocher
  du modèle, a été fermée par le robot ; voir RELEASE.md).
- **Vitrine** : https://adrienavalon.github.io/avash/ (et `/en/`), publiée par
  GitHub Pages à chaque changement de `site/` ; les fichiers de la dernière
  version y sont listés par système.

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
- **Mesuré et refusé (04/09/2026) : l'envoi SFTP en bandes parallèles.** Le
  symétrique du téléchargement a été écrit (huit descripteurs sur le même
  fichier distant, chacun à son décalage) et mesuré avec la même sonde contre
  un `internal-sftp` d'OpenSSH sur 8 Mio, la latence injectée par `netem` sur
  l'interface locale :

  | aller-retour | montée séquentielle | montée en bandes | descente séquentielle | descente en bandes |
  |---|---:|---:|---:|---:|
  | 0 ms (local) | 160,5 Mo/s | 37,7 Mo/s | 148,7 Mo/s | 253,6 Mo/s |
  | 40 ms | 10,9 Mo/s | 13,0 Mo/s | 1,5 Mo/s | 7,1 Mo/s |

  La montée séquentielle est déjà pipelinée par russh-sftp (huit écritures en
  vol) : les bandes y perdent quatre fois en local pour gagner 1,2 × à 40 ms.
  Pas retenu ; l'envoi reste séquentiel, avec une reprise par point de
  contrôle. La descente en bandes, elle, confirme ses 4,7 × à 40 ms.
- **Fait depuis (0.4.0)** : la boîte englobante des rectangles sales a laissé
  place à une fusion sélective, mesurée sur le fil — 8,39 Mo → 4,36 Mo pour le
  même parcours, avec davantage de trames livrées.
- **Fait (0.6.2)** : le panneau SFTP rouvrait une session SSH complète, chaîne
  de rebonds comprise, au lieu d'ouvrir un canal sur celle de l'onglet : cinq
  à sept allers-retours avant le moindre listing. Il ouvre son canal sur la
  session vivante (voir 1.4).
- **Repères de non-régression.** Les mesures existantes (regroupement, décodage
  UTF-8) affichent des chiffres mais rien ne cassait s'ils se dégradaient. Un
  test pose désormais un plancher sur le décodeur UTF-8 en flux, large (dix
  fois sous la mesure en profil de test) pour ne pas rougir sous charge, mais
  qui verrait une régression algorithmique.
- **Mesuré (04/09/2026), sur la vraie application** par
  `scripts/mesures-front.sh` (harnais bout en bout, machine au repos, cinq
  lancements, quarante frappes sur le sshd local) :

  | Mesure | Médiane | p95 / max |
  |---|---:|---:|
  | Exécution des modules JavaScript (domInteractive → DOMContentLoaded) | 189 ms | max 1 122 ms (premier lancement à froid) |
  | DOMContentLoaded depuis la navigation | 205 ms | max 1 155 ms |
  | Premier contenu peint | 101 ms | max 1 077 ms |
  | Frappe → écho reçu du serveur (keydown → événement `pty-output`) | 11 ms | p95 16 ms, max 19 ms |
  | Frappe → image suivante (keydown → `requestAnimationFrame` après l'écho) | 18 ms | p95 27 ms, max 29 ms |

  La latence à la frappe passe sous le cap de 16 ms pour l'écho ; l'image qui
  le montre vient une trame plus tard, à la cadence de l'écran. Rien à
  optimiser sur ce chemin.
- **Découpage du paquet front — mesuré puis fait (05/09/2026).** Un seul
  module de 666 Ko, dont xterm.js 331, l'application 149, le rendu WebGL 113,
  la recherche 32, l'API Tauri 19, la sérialisation 15 (mesuré par un build
  à un morceau par paquet). Deux repères `performance.mark` (premier module
  évalué, fin des imports) ont ensuite séparé les trois parts de l'exécution
  des modules : lecture et compilation du paquet 283 ms, évaluation 13 ms,
  initialisation d'avash 7 ms — c'est le poids du paquet qui coûte, pas son
  exécution. xterm.js et ses extensions sont donc chargés à part
  (`web/xterm-charge.ts`, imports dynamiques), à l'oisiveté juste après
  l'accueil ou au premier terminal. Après, cinq lancements, machine au repos :

  | Mesure | Avant | Après |
  |---|---:|---:|
  | Exécution des modules (domInteractive → DOMContentLoaded) | 303 ms | 184 ms |
  | dont lecture et compilation du paquet | 283 ms | 175 ms |
  | DOMContentLoaded depuis la navigation | 323 ms | 205 ms |
  | Premier contenu peint | 190 ms | 152 ms |
  | Frappe → écho reçu (médiane) | 11 ms | 10 ms |

  Le paquet principal fait 172 Ko. Le premier terminal ne paie rien de plus
  quand le chargement à l'oisiveté l'a précédé, ce qui est le cas courant.

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
  traversent. Remesurée après le lot du même jour : **75 %** des lignes pour
  cœur et interface (`commands.rs` 49 %, `rdp.rs` 38 %, outil en ligne de
  commande 69 %), 66 % pour le processus RDP, dont la boucle de session et la
  capture restent hors de portée des tests unitaires. À suivre à chaque
  version.
- **Contrôle des dépendances** — **fait** : `cargo-audit` pour les
  vulnérabilités connues, `cargo-deny` pour les licences, les jokers et les
  sources hors registre, `npm audit` sur les deux arbres Node, Dependabot pour
  les mises à jour, `cargo machete` de temps en temps pour les dépendances
  déclarées sans être utilisées (cinq retirées à sa première exécution).
- **Regards extérieurs sur le dépôt** — **fait** (02/09/2026) : CodeQL (Rust,
  TypeScript), gitleaks sur tout l'historique, Scorecard de l'OpenSSF, chaque
  lundi et à chaque poussée.
- **Force des tests, mesurée par mutation.** `cargo mutants` sur un échantillon
  de quatre modules du cœur : 42 mutants testés avant l'arrêt de l'essai, 29
  tués, 13 survivants — tous dans l'arithmétique du calendrier de
  l'enregistreur, que le test ne vérifiait qu'en forme. Un test contre des
  dates connues les tue. À rejouer par lots (l'outil doit tourner en place, le
  mode copie emporte 14 Go de compilation) sur les modules de sécurité en
  priorité : `ssh.rs`, `keys.rs`, le parseur de configuration.
- **Test par fuzzing** du parseur `~/.ssh/config` — **fait** (02/09/2026) : un
  test de mutation déterministe (2 000 variantes d'une configuration réaliste :
  octets retournés, troncatures, fragments de mots-clés, octets hostiles)
  vérifie qu'aucune ne fait paniquer le parseur et que ce qu'il rend reste
  cohérent. Première trouvaille dès l'écriture : un espace collé au nom d'hôte
  d'un `ProxyJump` traversait le découpage, et le rebond devenait introuvable
  sans que le message ne le montre. Poursuivi le soir même avec `cargo-fuzz`
  (nightly, couverture guidée) : cinq cibles dans `fuzz/` — config SSH,
  session PuTTY, registre, `MobaXterm.ini`, asciicast —, jouées 45 s chacune
  par le workflow Sécurité. Deux trouvailles en quelques secondes, là où
  2 000 mutations n'y arrivaient pas : `Port 0` accepté, qu'OpenSSH refuse ;
  et une panique du décodage des noms de session PuTTY sur `%` suivi d'un
  caractère multi-octets — un nom de fichier suffisait à faire tomber
  l'import. Le fuzzing guidé trouve ce que la mutation à graine fixe ne
  produit pas : à garder en chaîne, et à étendre à chaque nouveau parseur.
- **Scénarios bout en bout sous Windows** — **fait** (02/09/2026) : la même
  suite WebdriverIO joue à chaque poussée, sur l'exécutable Windows, tout ce
  qui ne demande pas de serveur local. Le chemin classique (tauri-driver puis
  Edge WebDriver) est mort avec Edge 133, qui ne lance plus une application
  WebView2 ; l'application embarque donc, pour la suite seulement
  (`--features webdriver`), un serveur WebDriver que le harnais lance et
  arrête à chaque fichier. Le même chemin joue la suite sous macOS, qui n'a
  aucun pilote : l'interface y est exercée pour la première fois.
  **Serveurs locaux compris depuis le 05/09/2026** : sept passages d'une
  branche d'essai, dont un instrumenté (traces horodatées dans le cœur, sshd
  en `DEBUG1`), ont trouvé ce que le serveur embarqué ne transmet pas au DOM
  (le double-clic, les caractères tapés dans un terminal), le rechargement de
  page qui coupe le pilotage, le chemin d'un enregistrement présumé
  commencer par `/`, et le dossier de téléchargement hors du bac à sable
  (`dirs::download_dir()` ignore `AVASH_HOME` sous Windows). La suite
  complète passe désormais sous Windows à chaque poussée, avec le sshd
  d'OpenSSH Server et les serveurs RDP et VNC de test ; seuls la
  réouverture des onglets et les vraies touches restent réservés à Linux.
- **Régression visuelle** — **fait** (02/09/2026) : quatre captures de
  référence produites par la chaîne, comparées pixel à pixel à chaque passage.
- **Un diagnostic exportable** — **fait** (05/09/2026) : « Exporter un
  diagnostic… » dans la palette écrit, dans un fichier choisi par
  l'utilisateur, ce qu'un ticket a besoin de savoir (versions, système,
  emballage, trousseau, agent, configuration en nombre, journaux des sessions
  de bureau distant) et rien de ce qu'il ne doit pas voir. Un ticket sans ces
  faits coûte un aller-retour ; un diagnostic qui recopierait la
  configuration coûterait plus cher.
- **Accessibilité au clavier** — **fait** : boîtes de dialogue (piège de focus,
  Échap), et liste d'hôtes parcourue aux flèches, Origine et Fin, avec un seul
  arrêt de tabulation et un focus qui vaut sélection, le tout sous scénarios
  bout en bout.

---

## Axe 5 — Fonctionnalités

Par ordre de valeur décroissante :

1. **Enregistrement de session** (format asciinema) — **fait** : depuis le menu du terminal, un fichier asciicast v2 par session dans le répertoire de configuration, en 0600 ; la sortie et les redimensionnements, jamais les frappes ; relu par un scénario bout en bout sur la session SSH réelle.
2. **Santé des hôtes** — **fait** : une connexion TCP jusqu'au port de chaque hôte SSH et RDP, seize à la fois, bornée à 1,5 s, lancée depuis la palette ; voyant vert ou rouge sur la ligne, latence ou raison en infobulle ; un hôte derrière un rebond n'est pas sondé. Scénario bout en bout sur le sshd local et une adresse sans route.
3. **VNC** — **fait** (04/09/2026) : RFB 3.8 par le même processus, le même
   canal et le même canvas que le RDP (client `vnc-rs` porté et durci dans
   `rdp-sidecar/vendor/`), clavier en keysyms, presse-papiers, mot de passe au
   trousseau ; un serveur VNC de test réagit aux entrées et le scénario bout
   en bout mesure les pixels (clic → carré magenta, « g » → bureau vert,
   « é » → keysym 0xe9). Pas de chiffrement : dit dans le formulaire et dans
   `SECURITY.md`, tunnel SSH recommandé ; VeNCrypt (TLS, certificat épinglé)
   depuis le 05/09/2026. Reste à faire : le pseudo-codage curseur, Tight
   (JPEG).
4. **Port série** — **fait** (05/09/2026) : mode Série de la connexion
   directe (ports du poste proposés, vitesse), session dans un onglet de
   terminal par les canaux des sessions SSH, deux fils bloquants sur le port.
   Test du cœur sur un pseudo-terminal `openpty`, scénario bout en bout sur
   un pseudo-terminal `socat` qui renvoie ce qu'il reçoit. Reste à faire :
   parité et bits d'arrêt au choix, mémoriser un port dans la barre latérale.
5. **Transfert de fichiers par RDP** — **fait** (04/09/2026), par le
   presse-papiers, comme mstsc : la liste des fichiers copiés sur le distant
   arrive dans une pastille, un clic et une confirmation les reçoivent (par
   morceaux, quatre en vol, sans écraser) ; des fichiers déposés sur le bureau
   se collent dans son Explorateur, dossiers compris. Le scénario bout en bout
   compare les octets dans les deux sens contre le serveur de test. La
   redirection de lecteur, pour parcourir un dossier du poste depuis le
   distant, est le point 11.
6. **Le panneau SFTP au complet** — **fait** (04/09/2026) : dossiers entiers
   dans les deux sens, reprise d'un transfert coupé ou annulé (carte des
   bandes reçues, point de contrôle des envois), file de transferts (trois à
   la fois, vitesse, annulation), copie d'un hôte à un autre par relais sans
   écrire sur le poste, ou directe par `scp` chez la source avec l'agent SSH
   prêté le temps de la commande. Six tests d'intégration sur un serveur SFTP
   de test doté d'un système de fichiers en mémoire, deux scénarios bout en
   bout contre le vrai sshd.
7. **Les onglets de la dernière fois** — **fait** (05/09/2026) : la liste des
   onglets ouverts (hôtes déclarés et bureaux enregistrés) suit chaque
   ouverture et fermeture dans `onglets.json` ; l'accueil propose de les
   rouvrir, dans l'ordre, ou d'oublier. Scénario bout en bout qui recharge le
   front sur le sshd local.
8. **Vue partagée** — **fait** (05/09/2026) : `Ctrl+Maj+E` met l'onglet actif et
   le suivant côte à côte, terminal ou bureau distant indifféremment ; l'affichage
   de la zone centrale passe par un seul module. Scénario bout en bout sur
   deux sessions SSH.
9. **VeNCrypt** — **fait** (05/09/2026) : TLS pour le VNC (sous-types X.509
   seulement), certificat épinglé au premier contact comme en RDP, refus
   avec les empreintes quand il change. Le serveur de test a son terminateur
   VeNCrypt ; deux scénarios bout en bout.
10. **Audio RDP** — **fait** (05/09/2026) : canal `rdpsnd` en PCM 16 bits
    seulement, blocs relayés à la webview (`[20]`) qui les joue par WebAudio,
    volume du serveur (`[21]`), coupure dans la palette. Scénario bout en bout
    sur le la 440 Hz du serveur de test.
11. **Redirection de lecteur (RDPDR)** — **fait** (05/09/2026) : un dossier
    du poste, choisi dans la fiche du bureau ou la connexion directe, servi au
    distant comme lecteur « Avash » (canal `rdpdr`, `disque.rs`, un fil dédié
    aux entrées-sorties, chemins confinés sous la racine). Le serveur de test
    a son côté serveur RDPDR ; le scénario bout en bout le voit énumérer, lire
    et écrire dans le dossier.

---

## Comment savoir si l'on progresse

Ces mesures sont à relever à chaque version :

| Indicateur | Aujourd'hui | Cap |
|---|---|---|
| Plateformes réellement livrées | 2, plus macOS construite mais non éprouvée | 3 éprouvées |
| Scénarios bout en bout | 69 | en hausse à chaque fonctionnalité |
| Couverture des tests | 75 % des lignes (cœur + interface), 66 % (processus RDP) | en hausse à chaque version |
| Latence à la frappe (SSH local) | 11 ms jusqu'à l'écho, 18 ms jusqu'à l'image (médianes, 04/09/2026) | < 16 ms, tenue |
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
Réglé le 3 septembre 2026 : un exécuteur tourne sur le poste du mainteneur et
la chaîne GitLab est verte de bout en bout, conformité RDP comprise. Son
premier passage réel a coûté douze commits : une image sans `rustfmt`, un
Node trop vieux, un pare-feu qui rejetait une ligne de journal, un parc RDP
injoignable en docker-in-docker… et, au passage, un vrai défaut du cœur
(l'écriture atomique resserrait les droits d'un répertoire existant) que
seul un exécuteur en root pouvait révéler. Le miroir vérifie ; le détail est
dans `CHANGELOG.md` et `CONTRIBUTING.md`.

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
