# Qualité : ce que les tests vérifient, et comment

Ce document détaille la section « Qualité » du [README](../README.md) : les
niveaux de tests, leurs compteurs, et les trois dispositifs qui ont fait leurs
preuves (juge d'accessibilité extérieur, rejeu d'enregistrements réels,
conformité RDP contre de vrais serveurs).


**1164 tests** couvrent le projet, tous exécutés à chaque commit :

| Niveau | Nombre | Ce qui est vérifié |
|---|---|---|
| Cœur (`crates/avash`) | 155 | parseur `~/.ssh/config` et son **fuzzing par mutation** (plus sept cibles cargo-fuzz dans `fuzz/`, dont le décodeur ClearCodec du canal graphique et le flux entier d'un serveur VNC), import PuTTY et MobaXterm, enregistrement asciicast, sonde de santé, clés d'hôte, secrets, dossiers, tunnels, snippets, écritures atomiques, clés générées privées dès leur création |
| Intégration | 42 | contre un **vrai serveur SSH** : authentification et ses refus, PTY, SFTP sur la session du terminal (dossiers récursifs dans les deux sens, reprise après annulation, relais entre deux serveurs, sur un système de fichiers en mémoire), tunnels, rebonds `ProxyJump` ; l'outil en ligne de commande exercé comme binaire |
| Interface (`crates/avash-ui`) | 70 | commandes Tauri, import de sessions, enregistrement, santé des hôtes, magasin de sessions sur moteur factice (annulation pendant la connexion, éviction par époque), résolution des rebonds `ProxyJump`, décodage UTF-8 en flux, verrous clavier, annonce du processus RDP, variables d'environnement de la webview |
| Processus RDP | 122 | son du distant (formats PCM, message, relais), fichiers par le presse-papiers (réception par morceaux dans le désordre, refus, réponse courte, offre et parcours des dossiers), session VNC (entrées, masque de boutons, copie de rectangles), empreinte du serveur, fichier des empreintes, écriture atomique, plafond de résolution, négociation, identifiants et domaine, format binaire des trames, nouvelle taille d'écran sans image vidée, configuration après redirection, origine WebSocket, disposition clavier, isolation des tests, zone sale, **résistance aux messages malformés**, canal graphique (surfaces bornées, image refusée hors surface, cache, ClearCodec, RemoteFX Progressive : décodeur SRL, paliers d'affinage, tuiles en différence, tuile hors surface sans état), magnétoscope, rejeu d'enregistrements réels (icônes NSCodec non noires), fuzzing par mutation sur cinq enregistrements |
| Paquets IronRDP et vnc-rs portés | 595 | nos correctifs — remplissage des tuiles, bande passante, redirection de serveur, capacités précoces, **ordre des champs de ClearCodec**, RLEX à une couleur, **sous-codec NSCodec**, et pour le client VNC un serveur hostile scénarisé (allocations bornées, résultat d'authentification, rectangle hors cadre, refus sans raison) — et les tests amont de `ironrdp-pdu` et `ironrdp-graphics`, qui ne s'exécutaient nulle part (voir [rdp-sidecar/vendor](../rdp-sidecar/vendor/README.md)) |
| Front (Vitest) | 115 | logique pure : arborescence, chemins de dossiers, filtres, scancodes, keysyms VNC, mappage souris, réglages, collage sûr, traductions (couverture des deux dictionnaires, variables, page) |
| Bout en bout (WebdriverIO) | 65 | l'application réelle : connexions SSH, RDP et VNC effectives, SFTP, enregistrement asciicast, santé des hôtes, presse-papiers RDP, dossiers, import PuTTY, langue, modales, tunnels, snippets, accessibilité, navigation au clavier, **audit axe-core sur les deux thèmes** — tous en intégration continue, serveurs locaux compris |

S'y ajoutent `clippy` en mode strict — **en profil debug et en profil release**,
qui ne voient pas le même code — ESLint typé, stylelint, knip (code mort),
`cargo audit`, `cargo deny` et `npm audit` sur tous les arbres de dépendances,
et une garde qui interdit les motifs dangereux. Sur le dépôt : CodeQL,
gitleaks, le Scorecard de l'OpenSSF et Dependabot (voir
[CONTRIBUTING.md](../CONTRIBUTING.md)). Deux chaînes indépendantes jouent tout
cela à chaque poussée : GitHub Actions (Linux, Windows, macOS) et le miroir
GitLab, sur un exécuteur du mainteneur (Linux, conformité RDP contre de vrais
serveurs xrdp comprise).

### Accessibilité : un juge extérieur

Les vérifications écrites à la main couvrent ce à quoi on a pensé — rôles des
modales, piège à focus, retour du focus. `axe-core` couvre ce à quoi on n'a pas
pensé, et il a trouvé du premier coup : un texte secondaire à **3,15:1** au lieu
de 4,5, des initiales d'avatar à 4,44, un champ sans étiquette visible, un rôle
ARIA interdit sur un `<form>`. Le thème clair était **pire encore** — 2,45:1 —
et aucun test ne l'aurait montré : ils tournent tous en sombre.

Corrigé par le calcul, pas à l'œil : chaque couleur retenue tient 4,5:1 sur
*toutes* les surfaces où elle apparaît, et l'encre des initiales mêle la teinte
de l'hôte à la couleur de texte du thème, de sorte que la lisibilité suive
automatiquement.

### Rejouer un serveur disparu

Le dialogue d'un vrai serveur est capturé une fois, puis rejoué sans réseau :
**5 millisecondes contre 5 secondes de connexion**. Une machine du parc devient
une fixture permanente, et le rendu obtenu est comparé à une empreinte de
référence — en débranchant le correctif du cisaillement, elle change.

Surtout, ces enregistrements servent de graines à un **fuzzing par mutation**.
Muter des octets au hasard ne franchit jamais les premières validations ; muter
du trafic authentique atteint le décodeur d'images. Il y a trouvé deux façons
pour un serveur hostile de faire tomber le client — une écriture hors tampon et
un débordement arithmétique — l'une et l'autre corrigées. Détails dans
[SECURITY.md](../SECURITY.md).

### Conformité RDP : de vrais serveurs, pas des simulacres

Trois défauts RDP corrigés en 0.3.3 — image cisaillée en diagonale, clavier
interprété en QWERTY, connexion suspendue sans fin — ont **tous** été signalés
par l'usage, et **aucun** n'était visible depuis les tests. Les tests unitaires
vérifiaient nos fonctions, la suite bout en bout vérifiait l'interface ; entre
les deux se trouvait le seul endroit où ces défauts vivaient : le dialogue réel
avec un serveur RDP.

Un parc de serveurs en conteneur comble ce vide : deux bureaux xrdp — XFCE et
GNOME, parce qu'ils ne dessinent pas de la même façon — et un sshd qui refuse la
méthode `password`, pour éprouver le repli `keyboard-interactive` dont l'absence
empêchait tout compte de domaine de se connecter.

```bash
scripts/parc-rdp.sh up tous        # XFCE 3390, GNOME 3391, sshd 2222
scripts/conformite.sh tous         # connexion, image, trafic, clavier, SSH, SFTP
scripts/parc-rdp.sh down
```

Le détecteur de cisaillement est lui-même éprouvé : en désactivant le correctif
porté, il annonce `CISAILLÉE décalage=-2 (96% des lignes)` ; correctif remis,
`saine décalage=+0 (100%)`. Détails dans
[tests-parc/README.md](../tests-parc/README.md).

Une règle tient lieu de discipline : **un nouveau test doit avoir été vu
échouer**. On débranche ce qu'il couvre et on vérifie qu'il tombe — un test qui
ne peut pas échouer ne protège rien.

```bash
./check.sh              # tout valider
./check.sh --quick      # sans le build release
cd e2e && npm test      # tests bout en bout (ouvre des fenêtres)

# conformité RDP contre de vrais serveurs xrdp
scripts/parc-rdp.sh up tous && CONFORMITE_RDP=1 PARC=tous ./check.sh
```

