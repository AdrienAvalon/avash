# Notices des polices embarquées

`avash-mono-regular.woff2` et `avash-mono-bold.woff2` sont des sous-ensembles
WOFF2 de **MesloLGS Nerd Font Mono, Nerd Fonts 3.5.1**. Ils associent la police
Meslo et des jeux de glyphes tiers. Les notices ci-dessous conservent leurs
attributions et conditions d’origine ; elles ne changent pas la licence du
code d’Avash.

La notice [Meslo d’André Berg](LICENSE-MesloLGS-NerdFont.txt) reste
applicable à son périmètre. Le texte complet [Apache 2.0](licenses/APACHE-2.0.txt)
et la notice du projet [Nerd Fonts](licenses/NERD-FONTS.txt) sont joints.
Les tables de nom des fichiers conservent également leurs mentions historiques
Apple, Tavmjong Bah et Bitstream.

## Jeux de glyphes identifiés

Les plages ci-dessous sont celles présentes dans ces deux sous-ensembles,
rapprochées des tables du `font-patcher` de Nerd Fonts 3.5.1. Elles ne décrivent
pas tous les glyphes proposés par chaque projet amont.

| Composant | Glyphes ou plages présents | Notice originale |
|---|---|---|
| Devicon, assemblage Nerd Fonts | U+E700–U+E8FF | [MIT, Devicon](licenses/DEVICON.txt) |
| Font Awesome, intégration Nerd Fonts | U+F000–U+F2FF | [Font Awesome Free, périmètres et conditions d’origine](licenses/FONT-AWESOME.txt) |
| Font Awesome Extension | U+E200–U+E2A9 | [MIT, André Luiz Gava](licenses/FONT-AWESOME-EXTENSION.txt) |
| Font Logos 1.4.0 | U+F300–U+F385 | [Unlicense](licenses/FONT-LOGOS.txt) |
| Octicons, intégration Nerd Fonts | U+F400–U+F533, U+2665 et U+26A1 | [MIT, GitHub](licenses/OCTICONS.txt) |
| Pomicons | U+E000–U+E00A | [SIL OFL 1.1, Gabriele Lana](licenses/POMICONS.txt) |
| Powerline Symbols | U+E0A0–U+E0A2, U+E0B0–U+E0B3 | [Notice Powerline Symbols](licenses/POWERLINE-SYMBOLS.txt) |
| Powerline Extra Symbols | plages U+E0A3–U+E0D7 définies par l’amont, et U+2630 | [MIT, Ryan L McIntyre](licenses/POWERLINE-EXTRA.txt) |
| Unicode IEC Power Symbols | U+23FB–U+23FE et U+2B58 | [MIT, Joe Loughry](licenses/UNICODE-IEC.txt) |

La notice Font Awesome Free contient plusieurs périmètres selon les éléments
concernés. Elle est reproduite intégralement ; ses conditions ne sont pas
remplacées par l’étiquette Apache de la police Meslo de base. Les notices OFL
conservent notamment les noms réservés déclarés par leurs auteurs.

Les logos de produits présents dans Devicon et Font Logos restent les marques
de leurs propriétaires. Leur usage sert à identifier ces produits et
n’implique aucune approbation d’Avash par ces propriétaires ; les politiques
de marque applicables restent à respecter.

## Transformation et provenance

Le projet Avash a réduit les plages Unicode puis converti les fichiers en
WOFF2, selon la [recette locale](README.md). Pour les versions ci-dessous,
les contours décomposés et avances de tous les caractères conservés
correspondent aux TTF Meslo Mono de Nerd Fonts 3.5.1. Le format, les tables
et les caractères supprimés rendent les fichiers binaires différents.

| Fichier | SHA-256 |
|---|---|
| `avash-mono-regular.woff2` | `b3a07a9c91b3f9cae49efb85f6242b18cc85af0787a63b36963b6b0897f032fd` |
| `avash-mono-bold.woff2` | `d4ecf2b122ea2b813143ee207b30e03897f6169caeb814676e9637a23154bb3d` |

[SOURCES.json](licenses/SOURCES.json) donne les URL amont épinglées et les
empreintes des notices, copiées sans modification. Cet inventaire concerne
ces fichiers précis ; il ne constitue pas une déclaration globale sur les
licences de toutes les dépendances, captures, logos ou futures polices d’Avash.
