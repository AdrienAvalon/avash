# Police embarquée

`avash-mono-{regular,bold}.woff2` — **MesloLGS Nerd Font Mono** (Nerd Fonts 3.5.1),
sous licence **Apache-2.0** (voir `LICENSE-MesloLGS-NerdFont.txt`).
Source : https://github.com/ryanoasis/nerd-fonts

## Pourquoi l'embarquer

Avash doit avoir le même rendu sur Linux et sur Windows. S'appuyer sur une
police système donnerait un résultat différent selon la machine — et sur
Windows, aucune police par défaut ne contient les glyphes powerline ni les
icônes utilisées par les invites de shell modernes (fish, starship,
powerlevel10k) : elles s'afficheraient en carrés vides.

## Comment elle a été produite

Le fichier d'origine fait 2,9 Mo par graisse. Il est réduit aux plages
réellement utiles à un terminal, puis compressé :

    RANGES="U+0000-00FF,U+0100-017F,U+0180-024F,U+0250-02AF,U+0370-03FF,\
    U+0400-04FF,U+2000-206F,U+2070-209F,U+20A0-20CF,U+2100-214F,U+2190-21FF,\
    U+2200-22FF,U+2300-23FF,U+2500-257F,U+2580-259F,U+25A0-25FF,U+2600-26FF,\
    U+2700-27BF,U+2B00-2BFF,U+E000-E0FF,U+E200-E2FF,U+E700-E8FF,U+F000-F3FF,\
    U+F400-F533"

    pyftsubset <source.ttf> --unicodes="$RANGES" --output-file=avash-mono-X.ttf
    woff2_compress avash-mono-X.ttf

Résultat : 2,9 Mo → environ 476 Ko par graisse.

Couverture : latin accentué, cyrillique, grec, ponctuation, flèches, maths,
semi-graphiques, blocs, formes, symboles, dingbats (dont `❯`), powerline
(U+E0A0-E0D4) et les icônes Nerd Font courantes.
