# Journal des modifications

Toutes les modifications notables d'Avash sont consignées dans ce fichier.

Le format s'inspire de [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/),
et le projet suit le [versionnage sémantique](https://semver.org/lang/fr/).

## [Non publié]

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

[Non publié]: https://github.com/AdrienAvalon/avash/compare/v0.2.7...HEAD
[0.2.0]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.0
[0.2.1]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.1
[0.2.2]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.2
[0.2.3]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.3
[0.2.4]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.4
[0.2.5]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.5
[0.2.6]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.6
[0.2.7]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.7
