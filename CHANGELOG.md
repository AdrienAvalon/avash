# Journal des modifications

Toutes les modifications notables d'Avash sont consignées dans ce fichier.

Le format s'inspire de [Keep a Changelog](https://keepachangelog.com/fr/1.1.0/),
et le projet suit le [versionnage sémantique](https://semver.org/lang/fr/).

## [Non publié]

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

[Non publié]: https://github.com/AdrienAvalon/avash/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/AdrienAvalon/avash/releases/tag/v0.2.0
