# Parc RDP local

De vrais serveurs xrdp, en conteneur, pour éprouver le rendu et les entrées
contre autre chose qu'un simulacre.

## Pourquoi

Les trois défauts RDP corrigés en 0.3.3 ont tous été signalés par Adrien en
utilisant avash, et **aucun n'était visible depuis les tests** :

| Défaut | Ce que voyaient les tests |
|---|---|
| image cisaillée en diagonale sur xrdp | rien : aucun test ne regardait une image |
| clavier interprété en QWERTY | rien : aucun test n'envoyait de touche à un vrai serveur |
| connexion suspendue sans fin | rien : aucun test ne menait une séquence complète |
| SSH : compte de domaine impossible | rien : le simulacre PAM était le nôtre, pas un vrai sshd |

Les tests unitaires vérifiaient nos fonctions, la suite bout en bout vérifiait
l'interface, et entre les deux se trouvait le seul endroit où ces défauts
vivaient : le dialogue réel avec un serveur RDP.

## Usage

    scripts/parc-rdp.sh up tous        # XFCE (3390), GNOME (3391), sshd (2222)
    scripts/conformite.sh tous         # les cinq contrôles
    scripts/parc-rdp.sh down           # nettoie

Ou intégré à la vérification complète :

    scripts/parc-rdp.sh up tous && CONFORMITE_RDP=1 PARC=tous ./check.sh

## Les trois serveurs

| Conteneur | Port | Ce qu'il joue |
|---|---|---|
| `xfce` | 3390 | xrdp + XFCE, refuse NLA — le cas SLED-15 |
| `gnome` | 3391 | xrdp + GNOME, autre toolkit donc autre rendu |
| `ssh` | 2222 | sshd qui **refuse** `password` et n'accepte que `keyboard-interactive`, et sert aussi SFTP |

Le serveur SSH mérite un mot : c'est le comportement d'un hôte joint à un
annuaire, où SSSD répond par une conversation PAM. avash n'avait pas ce repli,
et un compte de domaine ne pouvait donc pas se connecter. Le défaut a été
signalé depuis Windows, par l'usage. Contrôle négatif fait : en débranchant le
repli, le test échoue avec le message exact que voyait l'utilisateur —
« Authentification échouée pour essai. Le serveur propose encore :
keyboard-interactive. »

## Les deux bureaux

XFCE et GNOME ne dessinent pas de la même façon : boîtes à outils, polices et
stratégies de rafraîchissement diffèrent, donc les tuiles envoyées diffèrent.
C'est cette diversité qui fait sortir les défauts de décodage — le cisaillement
n'apparaît que lorsque le serveur complète ses tuiles à un multiple de quatre.

## Les contrôles

1. **La connexion aboutit**, et on mesure en combien de temps. Une connexion qui
   n'aboutit jamais était le symptôme le plus coûteux : l'onglet restait noir,
   sans un mot.
2. **L'image n'est pas cisaillée**, via `detecteur-cisaillement.py`. Une image
   cisaillée reste « plausible » — bonnes couleurs, bonne disposition — donc
   aucune vérification grossière ne l'aurait vue.
3. **La disposition clavier annoncée n'est pas zéro.** Annoncer zéro fait
   retomber xrdp sur un clavier américain.
4. **Les trames ne portent pas un plein écran pour deux poussières.** Le
   contrôle ne fixe pas de seuil en octets, qui serait fragile : il vérifie
   qu'une trame porte *plusieurs* rectangles. Un retour à l'union englobante
   donnerait exactement autant de rectangles que de trames.
5. **Le repli SSH clavier-interactif aboutit**, contre un serveur qui refuse la
   méthode `password`.
6. **SFTP fait l'aller-retour à l'octet près** — dépôt, relecture, comparaison,
   effacement, puis vérification que le fichier effacé ne se télécharge plus.
   Les tests d'intégration parlent à un serveur monté en mémoire, c'est-à-dire à
   notre propre compréhension du protocole ; ici, c'est un vrai OpenSSH.

## Ce détecteur est-il sérieux ?

Oui, et c'est vérifiable. En désactivant le correctif porté sur `ironrdp-session`
puis en relançant contre le conteneur XFCE :

    CISAILLÉE  décalage=-2 (96% des lignes)
    saine      décalage=+0 (100% des lignes)

Le parc reproduit donc le défaut réel, et le détecteur le distingue franchement.

## Le magnétoscope : rejouer un serveur disparu

`enregistrements/*.rec` contiennent le **dialogue authentique** de serveurs
réels — deux xrdp et un GNOME Remote Desktop —, capturé une fois puis figé. `avash-rdp --rejouer <fichier>` le repasse
dans le décodeur, sans réseau, sans TLS, sans NLA.

    scripts/parc-rdp.sh up xfce
    rdp-sidecar/target/release/avash-rdp --host 127.0.0.1 --port 3390 \
      -u essai -p essai-mot-de-passe --sans-nla --width 640 --height 480 \
      --enregistrer tests-parc/enregistrements/xfce.rec --shot /tmp/e.png

    rdp-sidecar/target/release/avash-rdp --rejouer tests-parc/enregistrements/xfce.rec

Trois usages, et le troisième est le plus important :

1. **Un serveur devient une fixture permanente.** Le comportement singulier
   d'une machine reste éprouvé même quand la machine a disparu.
2. **Les tests deviennent instantanés.** 5 millisecondes contre 5 secondes de
   connexion réelle — mille fois plus vite. Vérifié : en débranchant le
   correctif du remplissage des tuiles, l'empreinte du rendu passe de
   `df04a5d714c2a784` à `3a5ac9ea470a6a13`. Le rejeu voit donc le cisaillement,
   celui qu'il avait fallu un signalement d'utilisateur pour découvrir.
3. **Le fuzzing part de trafic réel.** Muter des octets au hasard ne franchit
   jamais les premières validations. Muter un enregistrement authentique atteint
   le décodeur d'images — et y a trouvé **quatre façons pour un serveur de faire
   tomber le client**, corrigées depuis (voir `rdp-sidecar/vendor/README.md`).
   Les deux dernières se trouvent sur le chemin graphique, dans la
   décompression ZGFX et la conversion des couleurs : c'est en étendant la
   campagne à l'enregistrement GNOME Remote Desktop qu'elles sont sorties.

Campagne longue à la demande :

    AVASH_FUZZ_TOURS=6000 cargo test un_serveur_hostile --manifest-path rdp-sidecar/Cargo.toml

## Ce que ce parc ne couvre PAS

Autant le dire que le découvrir plus tard.

- **GNOME Remote Desktop.** Le conteneur `gnome` est un *xrdp qui sert un bureau
  GNOME* — ce n'est pas la même chose. GNOME Remote Desktop est un serveur RDP à
  part entière : c'est lui qui exige EGFX et redirige vers la session de
  l'utilisateur, et c'est contre lui qu'avash a échoué le plus longtemps.
  Tentative faite, sans succès : son démon n'ouvre aucun port sans compositeur
  ni sortie virtuelle, donc sans session GNOME complète. Ce qui reste : le
  rejeu d'une session réelle enregistrée, qui couvre tout le décodage hors
  ligne, et le parc réel d'Adrien pour la vérification de bout en bout — TLS,
  redirection et RDSTLS compris, qu'un enregistrement ne rejoue pas.
- **La lecture du flux RDP au fil.** `tcpdump` et `tshark` sont installés mais
  RDP est chiffré dès la négociation : ils ne montrent que du TLS. C'est le
  **magnétoscope** qui joue ce rôle — il capture au niveau des PDU décodés,
  après TLS, et rejoue hors ligne.
- **Windows.** Aucun serveur RDP Windows dans le parc ; la conformité repose là
  encore sur les machines réelles. C'est la raison pour laquelle le canal
  graphique est refusé par défaut et n'est accordé qu'aux serveurs qui ont
  montré n'avoir que celui-là : faute de pouvoir éprouver ici ce qu'un Windows
  enverrait dessus, on ne le lui propose pas. Vérifié sur deux Windows du parc
  réel, image par image, contre la version publiée précédente.

## GNOME Remote Desktop : l'architecture, enfin comprise

Documentée par SUSE — l'éditeur de la SLED du parc réel — dans
[Headless remote sessions in GNOME][suse2]. Elle explique tout ce qu'on
observait sans le comprendre.

Le mode « headless » repose sur **deux démons** :

1. un **démon système** qui écoute sur le port RDP et ne montre aucun bureau ;
2. un **démon de transfert**, lancé par GDM dans une session d'écran de
   connexion, auquel le premier remet la connexion.

Et le transfert se fait précisément par le **PDU de redirection** que nous avons
décodé. Le client reçoit :

- un jeton de routage (`Cookie: msts=…`) ;
- des **identifiants à usage unique**, engendrés au hasard — ce qui explique le
  nom d'utilisateur incompréhensible qu'on avait lu (`69<;349v]V"0bW8<`) ;
- un certificat X.509 pour vérifier le serveur d'arrivée.

Le client doit alors **se déconnecter et se reconnecter** en présentant le jeton,
avec les identifiants à usage unique.

### Ce que cela impliquait pour avash — et qui est fait

Deux morceaux, dans cet ordre, tous deux en place depuis la 0.5.0 :

1. **Suivre la redirection** : rouvrir une connexion avec le jeton de routage
   dans la requête X.224 et les identifiants reçus, puis mener l'échange RDSTLS
   qui seul sait les consommer.
2. **EGFX**, sans quoi l'écran resterait vide même une fois redirigé — avec le
   codec RemoteFX Progressive, seul retenu par ces serveurs quand le client
   n'annonce pas H.264.

Une leçon de mise au point vaut d'être notée. Notre annonce de capacités
graphiques portait l'identifiant `0x0011` au lieu de `0x0012` : c'est celui de
`CacheImportReply`. Le message était donc parfaitement formé, et le serveur le
lisait sans broncher — il parlait simplement d'autre chose. Faute de recevoir
des capacités, GRD attendait dix secondes puis fermait la session sur
`BadCapabilities`, une erreur qui désignait la bonne famille de problème et la
mauvaise cause. Ce qui a tranché : capturer une session **FreeRDP** — le client
que Remmina emploie, et qui réussit — puis la déchiffrer et comparer nos octets
aux siens. La méthode vaut mieux que le correctif : quand un serveur reste muet
sans expliquer pourquoi, on met à côté un client qui, lui, obtient une réponse.

### Pourquoi le conteneur a échoué

J'essayais le mauvais mode. Le mode `--headless` d'un démon utilisateur ne sert
pas à cela : c'est le mode **système**, avec GDM, qui écoute et redirige. Sans
GDM, le démon annonce « RDP server started » et ne se lie à rien — ce qui est
exactement ce qu'on observait.

Un conteneur reproduisant cette architecture demanderait donc GDM, systemd et
une session complète. C'est possible, ce n'est pas petit.

[suse2]: https://www.suse.com/c/headless-remote-sessions-in-gnome-part-2/

## Chantier : GNOME Remote Desktop en conteneur

`Containerfile.grd.chantier` et `demarrer-grd.sh.chantier` sont **inachevés** —
le suffixe est là pour qu'on ne les prenne pas pour un serveur du parc. Ils
visaient le mode utilisateur, qui n'est pas le bon (voir ci-dessus) ; ils
restent utiles pour la partie compositeur.

Ce qui marche :

- `mutter --headless --virtual-monitor 1280x800` démarre et crée sa sortie
  (`Added virtual monitor Meta-0`). Il lui faut `/tmp/.X11-unix` inscriptible,
  faute de quoi il meurt sur Xwayland ;
- PipeWire et WirePlumber tournent ; `~/.local/state` doit exister et
  appartenir au compte ;
- le démon GRD démarre et annonce « RDP server started ».

Le message « RDP server certificate is invalid » était une **fausse piste** :
GRD calcule bien l'empreinte du certificat, l'erreur vient de la validation de
la paire tant que la clé n'est pas encore posée. Une fois les deux en place, le
message disparaît — et le serveur ne se lie toujours pas, parce que le mode
utilisateur n'est pas celui qui écoute.

Tant que ce point n'est pas levé, la vérification contre GNOME Remote Desktop
repose sur les machines réelles — mais plus seulement. Une session réelle est
**enregistrée au magnétoscope** dans `enregistrements/gnome-remote-desktop.rec`
et rejouée à chaque `cargo test` : le canal graphique s'ouvre, le flux
RemoteFX Progressive est décodé, et l'empreinte du rendu est figée. Une
régression du décodage se voit donc sans réseau ni serveur, comme pour `xfce` et
`gnome`. Le même enregistrement alimente le fuzzing par mutation, qui y a déjà
trouvé deux paniques déclenchables à distance.
