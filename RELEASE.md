# Construire et distribuer Avash

Objectif : **un fichier par système**, déployable par simple copie, vérifiable,
et — côté Windows — signé pour éviter les alertes.

| Système | Artefact | Déploiement | Dépendance à l'exécution |
|---|---|---|---|
| Linux | `Avash_<version>_amd64.AppImage` | copier le fichier, `chmod +x`, lancer | aucune (tout est empaqueté dedans) |
| Linux | `Avash_<version>_amd64.deb` | `apt install ./Avash_<version>_amd64.deb` | WebKitGTK 4.1 et GTK 3 du système (Debian 12, Ubuntu 22.04 et suivants) |
| Linux | `Avash-<version>-1.x86_64.rpm` | `dnf install ./Avash-<version>-1.x86_64.rpm` | idem (Fedora, openSUSE) |
| Windows | `Avash_<version>_x64-setup.exe` (NSIS) | lancer l'installeur | WebView2 (préinstallé Win10 récent / Win11) |
| Windows | `avash-<version>-windows-x64.zip` | décompresser et lancer, sans installation | WebView2 — garder `avash-rdp.exe` à côté d'`avash.exe` |

> L'AppImage embarque WebKitGTK et ses dépendances : c'est le seul artefact
> réellement « copier-coller et ça marche » sur une autre machine.
> Sous Windows, Tauri ne produit pas de binaire autonome — il s'appuie sur
> WebView2, présent par défaut sur les Windows actuels.

---

## 1. Prérequis

```
cargo install tauri-cli --version '^2.0' --locked   # une fois
# Linux : rien d'autre (Tauri télécharge appimagetool au premier build)
# Windows : Rust (MSVC), Node.js, et WebView2 SDK géré par Tauri
```

## 2. Build

Un seul point d'entrée, qui **valide puis construit** :

```
./scripts/release.sh                 # build + SHA256SUMS
./scripts/release.sh --sign-gpg <KEYID>   # + signature GPG (Linux recommandé)
```

- `check.sh` est exécuté d'abord : format, clippy strict (debug **et** release),
  tous les tests (les compteurs à jour sont dans `README.md`), `cargo audit` et
  `cargo deny` sur les deux arbres de dépendances, lint, typage, tests et audit
  du front. On ne publie pas du code non validé.
- Le **sidecar RDP** (`avash-rdp`, projet séparé hors workspace) est construit
  et déposé dans `crates/avash-ui/binaries/avash-rdp-<triple>` ; Tauri l'embarque
  via `externalBin` **à côté de l'exe** dans l'AppImage (le RDP marche donc dans
  le binaire distribué).
- `NO_STRIP=1` est passé au bundler : le `strip` embarqué par `linuxdeploy` ne
  gère pas la section `.relr.dyn` des bibliothèques système récentes (glibc/Arch
  moderne) et ferait échouer le build sinon.
- Les artefacts atterrissent dans `dist-release/` avec un fichier
  `SHA256SUMS` (et `SHA256SUMS.asc` si signé).

Le build Windows doit être lancé **sur une machine Windows** (ou un runner CI
Windows). La cross-compilation Linux→Windows avec Tauri est fragile et
déconseillée.

## 3. Vérifier l'intégrité (station d'analyse incluse)

Sur n'importe quelle machine, avant d'exécuter quoi que ce soit :

```
sha256sum -c SHA256SUMS            # les empreintes correspondent-elles ?
gpg --verify SHA256SUMS.asc SHA256SUMS   # authenticité (si signé GPG)
```

Sur une **station blanche / isolée** : copier l'artefact + `SHA256SUMS`,
recalculer l'empreinte hors ligne, la comparer. C'est la garantie qu'aucun
octet n'a changé entre la construction et l'analyse.

## 4. Signature de code Windows (anti-SmartScreen / antivirus)

C'est **le** point qui réduit les alertes. Il exige un **certificat de
signature de code** (Authenticode) à ton nom :

- **OV** (Organisation Validée) : ~200 €/an. Réduit les alertes ; la
  réputation SmartScreen se construit avec le nombre de téléchargements.
- **EV** (Extended Validation, sur token matériel) : plus cher, mais donne une
  **réputation SmartScreen immédiate** — l'option si tu veux zéro friction.

Une fois le certificat obtenu, deux voies :

**a) Via Tauri** — importe le certificat dans le magasin Windows, puis renseigne
son empreinte dans `tauri.conf.json` :
```json
"bundle": { "windows": { "certificateThumbprint": "<EMPREINTE_SHA1_DU_CERT>" } }
```
`cargo tauri build` signe alors automatiquement l'exe et l'installeur.

**b) À la main** — après le build, avec le SDK Windows :
```
signtool sign /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 \
  /a Avash_<version>_x64-setup.exe
signtool verify /pa Avash_<version>_x64-setup.exe
```

`timestampUrl` et `digestAlgorithm: sha256` sont déjà configurés : la signature
reste valide après expiration du certificat (horodatage).

> ⚠️ Un certificat **auto-signé ne sert à rien** ici : il n'est pas de
> confiance, l'alerte Windows persiste. Seul un certificat d'une autorité
> reconnue lève l'avertissement.

## 5. Faux positifs antivirus — attentes réalistes

Un binaire Rust/Tauri **non signé et neuf** peut déclencher 1 à 5 moteurs sur
~70 (VirusTotal), par **heuristique**, sans être malveillant. Ce qui réduit ce
risque, dans l'ordre :

1. **Signer** avec un certificat de confiance (section 4).
2. **Métadonnées propres** — éditeur, description, version : déjà renseignées
   dans `tauri.conf.json` (les binaires « anonymes » sont plus suspects).
3. **Pas de packing** — Avash n'utilise pas UPX ni d'obfuscation (ce sont des
   signaux que les AV pénalisent). Ne pas en ajouter.
4. **Réputation** — le compte de téléchargements et l'ancienneté font baisser
   les détections avec le temps.

En cas de faux positif persistant : soumettre l'artefact au formulaire de
**faux positif** de l'éditeur AV concerné (Microsoft Defender, etc.) avec le
SHA-256 ; la détection est en général retirée sous quelques jours.

**Aucune configuration ne garantit 0 détection.** Ce qu'on garantit ici :
un binaire **vérifiable** (checksums + signature), **traçable** (métadonnées),
**auditable** (`cargo audit`, sources), et **signable** dès que le certificat
est fourni.

## 6. Ce que le dépôt fournit déjà

- `tauri.conf.json` : cibles AppImage, deb, rpm et NSIS, métadonnées,
  horodatage de signature, emplacement du certificat (à renseigner). Les
  dépendances du `.deb` (`libwebkit2gtk-4.1-0`, `libgtk-3-0`) et du `.rpm`
  (les mêmes, par nom de bibliothèque) sont posées par tauri-cli ; le
  processus RDP `avash-rdp` est installé dans `/usr/bin` à côté de l'exe.
- `.syft.yaml` : la nomenclature logicielle (SBOM, SPDX 2.3) jointe à chaque
  release, calculée sur les fichiers de verrouillage de ce qui est livré et
  attestée à son tour, liée aux binaires. Vérification :
  `gh attestation verify --predicate-type https://spdx.dev/Document/v2.3 <fichier> --repo AdrienAvalon/avash`.
- `scripts/release.sh` : build validé + `SHA256SUMS` + signature GPG optionnelle.
- `check.sh` : la porte qualité exécutée avant chaque release.
- `LICENSE` (**AGPL-3.0-or-later**), icônes multi-résolutions
  (`crates/avash-ui/icons/`).
- `.github/workflows/release.yml` : sur un tag `v*`, construit Linux, Windows
  et macOS, produit le manifeste `latest.json` signé, les empreintes
  `SHA256SUMS`, le SBOM et une **attestation de provenance Sigstore**, puis
  publie la release.

À fournir par toi : le **certificat Authenticode** (Windows) et, si tu veux
signer l'AppImage, une **clé GPG**.

> L'attestation Sigstore n'est **pas** de l'Authenticode : elle prouve d'où
> vient le binaire, elle ne lève pas l'avertissement de Windows.

## 7. Mises à jour automatiques

Avash embarque le plugin updater (Tauri). En cliquant sur la pastille de
version, l'app vérifie un manifeste distant et propose d'installer.

**En place et éprouvé en conditions réelles** — une version installée détecte la
suivante, la télécharge et redémarre :
- Plugins `updater` + `process`, permissions, UI de vérification.
- Clé publique de signature dans `tauri.conf.json` (`plugins.updater.pubkey`).
- Endpoint : `https://github.com/AdrienAvalon/avash/releases/latest/download/latest.json`.
- `bundle.createUpdaterArtifacts: true` — **indispensable** : sans lui, Tauri ne
  signe rien, même avec la clé. C'était l'une des trois causes d'une mise à jour
  qui échouait en silence.

**Clé de signature des updates** (minisign, générée le 29/08) :
- Privée : `~/.config/avash-release/updater.key` — **hors dépôt, à garder
  secrète**. C'est elle qui signe chaque artefact de mise à jour.
- Publique (dans la config) : `dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEY5M0RGQ0EyRUE1RjFFMzkKUldRNUhsL3Fvdnc5K2RCTHVLZklWLzYrazR5VDNoL1Q1UVBUWEVlMW9lTWVtMTBSWWVJemZ6VEEK`
- Pour en régénérer une : `cargo tauri signer generate -w <chemin>`, puis
  remplacer `pubkey` dans `tauri.conf.json`.

**Publier une mise à jour — la voie normale :**

1. Porter le numéro de version dans les six endroits qui le déclarent :
   `Cargo.toml` (workspace), les trois `Cargo.toml` de crates, `tauri.conf.json`,
   `web/package.json`, et `VERSION` dans `.github/workflows/release.yml`.
2. Renseigner le `CHANGELOG.md`.
3. `NO_STRIP=1 ./scripts/release.sh` — valide, construit, et **régénère la
   distribution locale**. Ne pas sauter cette étape : sans elle, on essaie la
   version publiée en ligne pendant que sa propre copie est périmée.
4. Poser le tag et le pousser : `git tag -a vX.Y.Z -m "…" && git push <remote> vX.Y.Z`.
   Le workflow fait le reste — les deux plateformes, le manifeste signé, les
   empreintes, l'attestation, la release.

Le workflow **échoue volontairement** si aucune signature n'est trouvée : un
manifeste sans signature ferait échouer la mise à jour sans rien dire, ce qui
est pire qu'une publication qui s'arrête.

**En local, hors workflow** (rarement utile) :
```
export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.config/avash-release/updater.key)"
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""   # si passphrase
cd crates/avash-ui && NO_STRIP=1 cargo tauri build
```
`scripts/release.sh` exporte déjà cette clé si elle est présente.

## 8. Canaux de distribution

### winget (Windows)

Le paquet est `AdrienCros.Avash` sur le dépôt communautaire
[microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) ; les
utilisateurs l'installent par `winget install AdrienCros.Avash`. Chaque version
publiée s'y soumet par une PR contenant quatre manifestes (version, installeur,
locales en-US et fr-FR), générés depuis la release GitHub :

```bash
scripts/winget-manifeste.sh 0.7.2      # écrit packaging/winget/AdrienCros.Avash/0.7.2/
```

Puis, sans cloner le dépôt (plusieurs centaines de mégaoctets), depuis le fork
`AdrienAvalon/winget-pkgs` : une branche `AdrienCros.Avash-<version>` créée sur
le `master` amont par l'API, les quatre fichiers déposés dans
`manifests/a/AdrienCros/Avash/<version>/`, et une PR titrée
`Update: AdrienCros.Avash to <version>` (`New package:` pour la première). Le
robot du dépôt valide le manifeste et installe réellement le paquet ; le
mainteneur doit avoir signé une fois le CLA de Microsoft (le robot le demande
dans la PR). Les manifestes de la première soumission sont commités dans
`packaging/winget/` pour référence.

Automatiser depuis le workflow Release est possible avec l'action
`vedantmgoyal9/winget-releaser` sur un exécuteur Windows ; elle exige un jeton
personnel (`public_repo`) capable de pousser sur le fork, à déposer en secret
`WINGET_TOKEN`. À faire quand une version aura été acceptée à la main.

### AUR (Arch Linux, CachyOS, Manjaro…)

Le paquet `avash` se construit depuis les sources de la version publiée :
`packaging/aur/avash/PKGBUILD` (front Vite, processus RDP, application Tauri,
outil en ligne de commande, icônes, `.desktop`, métadonnées AppStream).
Éprouvé sur ce poste par `makepkg` avant chaque publication. À chaque version :

```bash
cd packaging/aur/avash
sed -i "s/^pkgver=.*/pkgver=0.7.3/; s/^pkgrel=.*/pkgrel=1/" PKGBUILD
updpkgsums                          # remplace l'empreinte de l'archive (pacman-contrib)
BUILDDIR=/tmp/avash-makepkg PKGDEST=/tmp/avash-makepkg makepkg -f --noconfirm
makepkg --printsrcinfo > .SRCINFO   # obligatoire, l'AUR le lit à la place du PKGBUILD
```

`BUILDDIR` hors du dépôt, sinon cargo voit l'archive extraite sous
`packaging/aur/avash/src/` comme un membre égaré de l'espace de travail
(« current package believes it's in a workspace when it's not ») et
`prepare()` échoue ; une dizaine de minutes de construction.

Publication : un compte sur https://aur.archlinux.org avec une clé SSH, puis
le dépôt `ssh://aur@aur.archlinux.org/avash.git` où l'on pousse `PKGBUILD` et
`.SRCINFO` sur `master`. Les utilisateurs installent par `paru -S avash` (ou
`yay`). La première poussée crée le paquet.

### Homebrew (macOS)

Un cask `packaging/homebrew/avash.rb` (image disque, empreinte, `livecheck`
sur les releases GitHub). Il se soumet en PR au dépôt `Homebrew/homebrew-cask`
(`brew bump-cask-pr` depuis un Mac, ou à la main) ; l'application n'étant pas
notarisée, le cask porte un `caveats` qui explique le clic droit puis Ouvrir.

### Flathub (Linux)

Flathub exige une construction depuis les sources dans son bac à sable, pas
un emballage de l'AppImage. Tout est dans `packaging/flathub/` : le manifeste
`io.github.AdrienAvalon.avash.yml` (runtime GNOME 49, extensions Rust stable
et Node 22, construction hors ligne), le `.desktop`, et les sources figées
`cargo-sources.json`, `cargo-sources-rdp.json` et `node-sources.json`, à
régénérer à chaque version par `scripts/flathub-sources.sh` (générateurs de
flatpak-builder-tools). L'identifiant Flathub suit le dépôt GitHub, parce que
la vérification d'un identifiant `dev.avash.app` demanderait de prouver la
propriété du domaine `avash.dev` ; celui de Tauri reste `dev.avash.app` et
l'application l'enregistre sur D-Bus (`--own-name`). À chaque version :

```bash
scripts/flathub-sources.sh                          # régénère les trois JSON
sed -i "s/tag: v.*/tag: v0.8.0/; s/commit: .*/commit: \"$(git rev-parse v0.8.0^{commit})\"/" \
  packaging/flathub/io.github.AdrienAvalon.avash.yml
flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest \
  packaging/flathub/io.github.AdrienAvalon.avash.yml
flatpak-builder --user --install --force-clean build-flatpak \
  packaging/flathub/io.github.AdrienAvalon.avash.yml   # une vingtaine de minutes
flatpak run io.github.AdrienAvalon.avash
```

Le linter signale trois droits qui demandent une exception, à justifier dans
la PR de soumission (dépôt `flathub/flathub`, branche `new-pr`, le manifeste
et les JSON à la racine) : `--socket=ssh-auth` (l'agent SSH du poste, avec ses
clés, et prêté le temps d'une copie directe), `--filesystem=home`
(`~/.ssh/config`, clés, `known_hosts`, transferts SFTP et fichiers RDP dans
les deux sens) et `--own-name=dev.avash.app` (l'identifiant que Tauri
enregistre sur D-Bus). Les mises à jour suivantes se font par PR sur le dépôt
`flathub/io.github.AdrienAvalon.avash` que Flathub crée à l'acceptation.

**La soumission est un geste du mainteneur en personne.** Le modèle de PR de
Flathub porte une liste à cocher que son robot exige complète (une première
PR ouverte sans elle, la #10077 du 05/09/2026, a été fermée aussitôt), et sa
[politique sur l'IA générative](https://docs.flathub.org/docs/for-app-authors/requirements#generative-ai-policy)
demande de **déclarer le contenu produit par IA** (ici : une large part du
code et de l'empaquetage a été écrite avec Claude Code, sous relecture et
essai du mainteneur ; le manifeste et ses JSON compris) et **interdit qu'un
agent IA ouvre la PR ou y réponde**. Il faut aussi joindre une vidéo de
l'application en Flatpak sous Linux. La branche est prête sur le fork
(`AdrienAvalon/flathub`, branche `io.github.AdrienAvalon.avash`, quatre
fichiers) : ouvrir la PR contre `new-pr`, coller le modèle, cocher chaque
point avec la déclaration ci-dessus, joindre la vidéo, demander les trois
exceptions.
