#!/usr/bin/env bash
# Écrit les manifestes winget d'une version publiée (packaging/winget/
# AdrienCros.Avash/<version>/), à partir de l'installeur NSIS de la release
# GitHub. Ils se soumettent ensuite à microsoft/winget-pkgs (voir RELEASE.md,
# « winget »).
#
# L'empreinte écrite est calculée ici, sur l'installeur téléchargé, après
# vérification de son attestation de provenance (Sigstore, produite par
# release.yml sur le tag). Trouvé par l'audit du 12 septembre 2026
# (C-chaine-10) : elle était recopiée du SHA256SUMS de la release, fichier
# modifiable, ni signé ni attesté ; remplacer l'installeur et SHA256SUMS sur la
# page de release donnait un manifeste cohérent avec le binaire substitué.
# SHA256SUMS doit encore concorder : deux sources d'empreinte qui divergent
# arrêtent tout. Garde : scripts/tests/winget-manifeste-verifie-la-provenance.sh.
#
# Usage : scripts/winget-manifeste.sh <version>     (ex. 0.7.2)
set -euo pipefail
cd "$(dirname "$0")/.."
v="${1:?version attendue, ex. 0.7.2}"
id="AdrienCros.Avash"
dossier="packaging/winget/$id/$v"
depot="AdrienAvalon/avash"
installeur="Avash_${v}_x64-setup.exe"
url="https://github.com/$depot/releases/download/v$v/$installeur"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
gh release download "v$v" -R "$depot" -D "$tmp" -p "$installeur" -p SHA256SUMS >/dev/null
# Produit par le workflow Release de ce dépôt, sur ce tag, sur un exécuteur
# hébergé par GitHub : sinon, pas de manifeste.
if ! gh attestation verify "$tmp/$installeur" -R "$depot" \
     --signer-workflow "$depot/.github/workflows/release.yml" \
     --source-ref "refs/tags/v$v" --deny-self-hosted-runners >/dev/null; then
  echo "attestation de provenance refusée pour $installeur : manifeste non écrit" >&2
  exit 1
fi
somme="$(sha256sum "$tmp/$installeur" | cut -c1-64 | tr 'a-f' 'A-F')"
publiee="$(grep " $installeur\$" "$tmp/SHA256SUMS" | cut -c1-64 | tr 'a-f' 'A-F' || true)"
if [ "$somme" != "$publiee" ]; then
  echo "SHA256SUMS de la release (${publiee:-absent}) ne correspond pas à l'installeur attesté ($somme) : manifeste non écrit" >&2
  exit 1
fi
date="$(gh release view "v$v" -R "$depot" --json publishedAt --jq '.publishedAt[0:10]')"
mkdir -p "$dossier"

cat > "$dossier/$id.yaml" <<EOF
# yaml-language-server: \$schema=https://aka.ms/winget-manifest.version.1.12.0.schema.json
PackageIdentifier: $id
PackageVersion: $v
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.12.0
EOF

cat > "$dossier/$id.installer.yaml" <<EOF
# yaml-language-server: \$schema=https://aka.ms/winget-manifest.installer.1.12.0.schema.json
PackageIdentifier: $id
PackageVersion: $v
InstallerType: nullsoft
Scope: user
InstallModes:
- interactive
- silent
- silentWithProgress
UpgradeBehavior: install
ReleaseDate: $date
Installers:
- Architecture: x64
  InstallerUrl: $url
  InstallerSha256: $somme
  ProductCode: Avash
  AppsAndFeaturesEntries:
  - DisplayName: Avash
    Publisher: Adrien Cros
    DisplayVersion: $v
    ProductCode: Avash
ManifestType: installer
ManifestVersion: 1.12.0
EOF

cat > "$dossier/$id.locale.en-US.yaml" <<EOF
# yaml-language-server: \$schema=https://aka.ms/winget-manifest.defaultLocale.1.12.0.schema.json
PackageIdentifier: $id
PackageVersion: $v
PackageLocale: en-US
Publisher: Adrien Cros
PublisherUrl: https://github.com/AdrienAvalon
PublisherSupportUrl: https://github.com/AdrienAvalon/avash/issues
PackageName: Avash
PackageUrl: https://github.com/AdrienAvalon/avash
License: AGPL-3.0-or-later
LicenseUrl: https://github.com/AdrienAvalon/avash/blob/main/LICENSE
Copyright: Copyright (c) 2026 Adrien Cros
CopyrightUrl: https://github.com/AdrienAvalon/avash/blob/main/LICENSE
ShortDescription: Native, fast and secure SSH, RDP and VNC connection manager
Description: |-
  Avash brings your SSH terminals, your Windows remote desktops (RDP), your VNC desktops, your serial consoles and your file transfers (SFTP) into a single native application. It reads and writes your ~/.ssh/config as it is, keeps passwords in the system credential store, verifies host keys for SSH and RDP before any credential leaves, shares a local folder as a drive on the remote desktop, and imports PuTTY and MobaXterm sessions. Built with Tauri 2 and Rust; requires the WebView2 runtime, shipped with Windows 10 and 11.
Moniker: avash
Tags:
- ssh
- rdp
- vnc
- sftp
- terminal
- remote-desktop
- ssh-client
- rdp-client
- vnc-client
- putty
- mobaxterm
- tauri
- rust
ReleaseNotesUrl: https://github.com/AdrienAvalon/avash/releases/tag/v$v
Documentations:
- DocumentLabel: README
  DocumentUrl: https://github.com/AdrienAvalon/avash/blob/main/README.en.md
ManifestType: defaultLocale
ManifestVersion: 1.12.0
EOF

cat > "$dossier/$id.locale.fr-FR.yaml" <<EOF
# yaml-language-server: \$schema=https://aka.ms/winget-manifest.locale.1.12.0.schema.json
PackageIdentifier: $id
PackageVersion: $v
PackageLocale: fr-FR
Publisher: Adrien Cros
PublisherUrl: https://github.com/AdrienAvalon
PublisherSupportUrl: https://github.com/AdrienAvalon/avash/issues
PackageName: Avash
PackageUrl: https://github.com/AdrienAvalon/avash
License: AGPL-3.0-or-later
LicenseUrl: https://github.com/AdrienAvalon/avash/blob/main/LICENSE
Copyright: Copyright (c) 2026 Adrien Cros
ShortDescription: Gestionnaire de connexions SSH, RDP et VNC, natif, rapide, sécurisé
Description: |-
  Avash réunit vos terminaux SSH, vos bureaux distants Windows (RDP), vos bureaux VNC, vos consoles série et vos transferts de fichiers (SFTP) dans une seule application native. Il lit et écrit votre ~/.ssh/config tel quel, garde les mots de passe dans le gestionnaire d'identifiants du système, vérifie les clés d'hôte en SSH et en RDP avant le moindre identifiant, partage un dossier du poste comme lecteur sur le bureau distant, et importe les sessions PuTTY et MobaXterm. Construit avec Tauri 2 et Rust ; requiert le moteur WebView2, livré avec Windows 10 et 11.
Tags:
- ssh
- rdp
- vnc
- sftp
- terminal
- bureau-distant
ReleaseNotesUrl: https://github.com/AdrienAvalon/avash/releases/tag/v$v
Documentations:
- DocumentLabel: README
  DocumentUrl: https://github.com/AdrienAvalon/avash/blob/main/README.md
ManifestType: locale
ManifestVersion: 1.12.0
EOF

echo "manifestes écrits dans $dossier :"
ls -1 "$dossier"
