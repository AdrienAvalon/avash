#!/usr/bin/env bash
# Écrit les manifestes winget d'une version publiée (packaging/winget/
# AdrienCros.Avash/<version>/), à partir de l'installeur NSIS et de son
# empreinte dans le SHA256SUMS de la release GitHub. Ils se soumettent ensuite
# à microsoft/winget-pkgs (voir RELEASE.md, « winget »).
#
# Usage : scripts/winget-manifeste.sh <version>     (ex. 0.7.2)
set -euo pipefail
cd "$(dirname "$0")/.."
v="${1:?version attendue, ex. 0.7.2}"
id="AdrienCros.Avash"
dossier="packaging/winget/$id/$v"
url="https://github.com/AdrienAvalon/avash/releases/download/v$v/Avash_${v}_x64-setup.exe"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
gh release download "v$v" -D "$tmp" -p SHA256SUMS >/dev/null
somme="$(grep " Avash_${v}_x64-setup.exe\$" "$tmp/SHA256SUMS" | cut -c1-64 | tr 'a-f' 'A-F')"
[ ${#somme} -eq 64 ] || { echo "empreinte de Avash_${v}_x64-setup.exe introuvable dans SHA256SUMS" >&2; exit 1; }
date="$(gh release view "v$v" --json publishedAt --jq '.publishedAt[0:10]')"
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
ReleaseNotesUrl: https://github.com/AdrienAvalon/avash/releases/tag/v$v
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
ShortDescription: Native, fast and secure SSH and RDP connection manager
Description: |-
  Avash brings your SSH terminals, your Windows remote desktops (RDP) and your file transfers (SFTP) into a single native application. It reads and writes your ~/.ssh/config as it is, keeps passwords in the system credential store, verifies host keys for SSH and RDP before any credential leaves, and imports PuTTY and MobaXterm sessions. Built with Tauri 2 and Rust; requires the WebView2 runtime, shipped with Windows 10 and 11.
Moniker: avash
Tags:
- ssh
- rdp
- sftp
- terminal
- remote-desktop
- ssh-client
- rdp-client
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
ShortDescription: Gestionnaire de connexions SSH et RDP, natif, rapide, sécurisé
Description: |-
  Avash réunit vos terminaux SSH, vos bureaux distants Windows (RDP) et vos transferts de fichiers (SFTP) dans une seule application native. Il lit et écrit votre ~/.ssh/config tel quel, garde les mots de passe dans le gestionnaire d'identifiants du système, vérifie les clés d'hôte en SSH et en RDP avant le moindre identifiant, et importe les sessions PuTTY et MobaXterm. Construit avec Tauri 2 et Rust ; requiert le moteur WebView2, livré avec Windows 10 et 11.
Tags:
- ssh
- rdp
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
