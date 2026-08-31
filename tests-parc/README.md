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

Les tests unitaires vérifiaient nos fonctions, la suite bout en bout vérifiait
l'interface, et entre les deux se trouvait le seul endroit où ces défauts
vivaient : le dialogue réel avec un serveur RDP.

## Usage

    scripts/parc-rdp.sh up tous        # démarre XFCE (3390) et GNOME (3391)
    scripts/conformite-rdp.sh tous     # les trois contrôles
    scripts/parc-rdp.sh down           # nettoie

Ou intégré à la vérification complète :

    scripts/parc-rdp.sh up tous && CONFORMITE_RDP=1 PARC=tous ./check.sh

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

## Ce détecteur est-il sérieux ?

Oui, et c'est vérifiable. En désactivant le correctif porté sur `ironrdp-session`
puis en relançant contre le conteneur XFCE :

    CISAILLÉE  décalage=-2 (96% des lignes)
    saine      décalage=+0 (100% des lignes)

Le parc reproduit donc le défaut réel, et le détecteur le distingue franchement.
