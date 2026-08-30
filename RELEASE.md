# Construire et distribuer Avash

Objectif : **un fichier par système**, déployable par simple copie, vérifiable,
et — côté Windows — signé pour éviter les alertes.

| Système | Artefact | Déploiement | Dépendance à l'exécution |
|---|---|---|---|
| Linux | `Avash_<version>_amd64.AppImage` | copier le fichier, `chmod +x`, lancer | aucune (tout est empaqueté dedans) |
| Windows | `Avash_<version>_x64-setup.exe` (NSIS) | lancer l'installeur | WebView2 (préinstallé Win10 récent / Win11) |

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

- `check.sh` est exécuté d'abord : format, clippy strict, tests (93 Rust + 51
  front), `cargo audit`. On ne publie pas du code non validé.
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

- `tauri.conf.json` : cibles AppImage + NSIS, métadonnées, horodatage de
  signature, emplacement du certificat (à renseigner).
- `scripts/release.sh` : build validé + `SHA256SUMS` + signature GPG optionnelle.
- `check.sh` : la porte qualité exécutée avant chaque release.
- `LICENSE` (MIT), icônes multi-résolutions (`crates/avash-ui/icons/`).

À fournir par toi : le **certificat Authenticode** (Windows) et, si tu veux
signer l'AppImage, une **clé GPG**.

## 7. Mises à jour automatiques

Avash embarque le plugin updater (Tauri). En cliquant sur la pastille de
version, l'app vérifie un manifeste distant et propose d'installer.

**Ce qui est déjà en place :**
- Plugins `updater` + `process`, permissions, UI de vérification.
- Clé publique de signature dans `tauri.conf.json` (`plugins.updater.pubkey`).
- Endpoint (à adapter) : `releases/latest/download/latest.json`.

**Clé de signature des updates** (minisign, générée le 29/08) :
- Privée : `~/.config/avash-release/updater.key` — **hors dépôt, à garder
  secrète**. C'est elle qui signe chaque artefact de mise à jour.
- Publique (dans la config) : `dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEY5M0RGQ0EyRUE1RjFFMzkKUldRNUhsL3Fvdnc5K2RCTHVLZklWLzYrazR5VDNoL1Q1UVBUWEVlMW9lTWVtMTBSWWVJemZ6VEEK`
- Pour en régénérer une : `cargo tauri signer generate -w <chemin>`, puis
  remplacer `pubkey` dans `tauri.conf.json`.

**Publier une mise à jour :**
1. Build en signant les artefacts updater :
   ```
   export TAURI_SIGNING_PRIVATE_KEY="$(cat ~/.config/avash-release/updater.key)"
   export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""   # si passphrase
   cd crates/avash-ui && cargo tauri build --config '{"bundle":{"createUpdaterArtifacts":true}}'
   ```
   (`createUpdaterArtifacts` n'est PAS activé par défaut pour ne pas exiger la
   clé à chaque build local.)
2. Publier les artefacts + leur `.sig` sur l'hébergement.
3. Générer/mettre à jour `latest.json` (version, notes, URLs + signatures) et
   le servir à l'endpoint configuré. Format : voir la doc Tauri updater.

Sans manifeste publié, le bouton affiche simplement « vérification
impossible » — c'est attendu tant que la première release n'est pas en ligne.
