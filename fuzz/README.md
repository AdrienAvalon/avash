# Fuzzing des parseurs du cœur et du canal graphique

Ce que le cœur lit depuis un fichier écrit par quelqu'un d'autre — l'utilisateur
à la main, un outil tiers, un dépôt de dotfiles, un enregistrement rapporté d'une
autre machine — et ce que le processus RDP décode depuis le serveur passent ici
sous un générateur guidé par la couverture
([cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz), libFuzzer, nightly).
Les tests de mutation déterministes — `crates/avash/src/lib.rs` pour le
parseur SSH, `rdp-sidecar/src/magnetoscope.rs` pour les cinq enregistrements
réels du canal graphique — restent la garde rapide, jouée à chaque commit ; le
fuzzing va plus loin, plus longtemps.

| Cible | Entrée | Ce qui doit tenir |
|---|---|---|
| `config_ssh` | `~/.ssh/config` | aucune panique ; alias jamais vide ni multiligne, port jamais nul, rebonds rognés ; tout bloc rendu par `render_host_block` se relit avec le même alias |
| `putty_session` | un fichier de `~/.putty/sessions/` et son nom encodé `%XX` | aucune panique ; une session acceptée a un alias, un hôte, un port non nul |
| `reg_query` | la sortie de `reg query` (registre Windows) | idem |
| `mobaxterm_ini` | `MobaXterm.ini` (`#109#` SSH, `#91#` bureaux RDP) | idem, et un bureau a un nom, un hôte, un port non nul |
| `asciicast` | un enregistrement asciicast v2 | aucune panique ; jamais plus d'événements que de lignes |
| `clearcodec` | deux images ClearCodec (MS-RDPEGFX 2.2.4.1) décodées à la suite par le même décodeur : couches résiduelle, bandes et sous-codecs (brut, NSCodec, RLEX), caches de glyphes et de barres | aucune panique ; une image acceptée a exactement `largeur × hauteur × 4` octets |

## Lancer

```bash
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz --locked

./fuzz.sh                 # chaque cible 60 s, depuis les graines
DUREE=600 ./fuzz.sh       # plus longtemps
cd fuzz && cargo +nightly fuzz run config_ssh corpus/config_ssh seeds/config_ssh   # une seule
```

`seeds/<cible>/` contient les graines commises (des entrées authentiques, tirées
des tests) ; `corpus/<cible>/` reçoit ce que libFuzzer découvre et reste local.
Un plantage dépose l'entrée fautive dans `artifacts/<cible>/` :

```bash
cd fuzz && cargo +nightly fuzz run config_ssh artifacts/config_ssh/crash-…   # rejouer
cargo +nightly fuzz fmt config_ssh artifacts/config_ssh/crash-…              # la voir
```

La chaîne (`Sécurité`, job `fuzz`) joue chaque cible 45 s à chaque poussée sur
`main` et chaque lundi. Ce crate est hors de l'espace de travail : il exige
nightly, et ne doit pas peser sur `cargo build` ni sur `check.sh`.
