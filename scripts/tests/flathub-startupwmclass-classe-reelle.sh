#!/usr/bin/env bash
# Le StartupWMClass du .desktop Flathub doit être la classe que la fenêtre émet
# vraiment, sinon le bureau (GNOME Shell, Plasma) ne relie pas la fenêtre à
# l'entrée .desktop : icône générique, épinglage de l'instance en cours cassé,
# regroupement perdu.
#
# Trouvé par l'audit du 9 septembre 2026 : le .desktop Flathub déclarait
# `StartupWMClass=dev.avash.app`, l'identifiant Tauri, alors que la fenêtre
# retombe sur le nom du binaire. Tauri ne transmet l'identifiant à GTK que si
# `enableGTKAppId` vaut true dans tauri.conf.json (tauri 2.11.5, src/app.rs :
# `let app_id = if manager.config.app.enable_gtk_app_id { Some(identifier) }
# else { None }` ; défaut false dans tauri-utils 2.9.3) ; cette clé est absente
# du fichier, donc tao reçoit `app_id: None`, `gtk::Application::new(None, …)`
# ne retient aucun nom, et WM_CLASS vaut le prgname, c'est-à-dire `avash-ui`.
# Le .desktop de l'AUR le faisait déjà correctement, pour le même exécutable.
#
# Le contrôle recalcule la classe attendue au lieu de la coder en dur : le jour
# où le drapeau passe à true, il exige l'identifiant, pas le binaire. La
# première version de ce contrôle promettait cela mais ne lisait qu'une des
# deux orthographes ; la relecture du 9 septembre 2026 l'a refusée pour ça.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

python3 - <<'PY'
import json
import pathlib
import re
import sys

import yaml

MANIFESTE = "packaging/flathub/io.github.AdrienAvalon.avash.yml"
DESKTOP = "packaging/flathub/io.github.AdrienAvalon.avash.desktop"
TAURI = "crates/avash-ui/tauri.conf.json"
PKGBUILD = "packaging/aur/avash/PKGBUILD"

manifeste = yaml.safe_load(open(MANIFESTE, encoding="utf-8"))
tauri = json.load(open(TAURI, encoding="utf-8"))
desktop = open(DESKTOP, encoding="utf-8").read()
pkgbuild = open(PKGBUILD, encoding="utf-8").read()

echecs = []

# tauri-utils 2.9.3 (src/config.rs) déclare le drapeau
# `#[serde(rename = "enableGTKAppId", alias = "enable-gtk-app-id", default)]` :
# les deux orthographes sont également valides dans tauri.conf.json, et
# `AppConfig` étant en `deny_unknown_fields`, ce sont les deux seules. Lire une
# seule des deux ramènerait le bug en silence, le contrôle restant vert.
ORTHOGRAPHES = ("enableGTKAppId", "enable-gtk-app-id")
app = tauri.get("app", {})
posees = [cle for cle in ORTHOGRAPHES if cle in app]
if len(posees) > 1:
    # serde refuse le doublon (le champ serait désérialisé deux fois) : tauri
    # ne construirait même pas. On le dit ici plutôt que de choisir un gagnant.
    echecs.append(
        f"{TAURI} : les deux orthographes {' et '.join(posees)} sont posées "
        f"ensemble ; serde rejette le doublon, tauri ne lira pas ce fichier"
    )

# La classe attendue : l'identifiant seulement si Tauri le donne à GTK, sinon
# le nom du binaire lancé (`command:` du manifeste, qui est aussi l'Exec).
commande = manifeste.get("command", "")
if any(app.get(cle, False) for cle in ORTHOGRAPHES):
    attendue = tauri.get("identifier", "")
    raison = f"{posees[0]}=true : GTK enregistre l'identifiant {attendue}"
else:
    attendue = commande
    raison = (
        "ni enableGTKAppId ni enable-gtk-app-id ne valent true dans "
        "tauri.conf.json : tauri passe app_id=None à tao, et WM_CLASS retombe "
        "sur le nom du binaire"
    )

trouvee = None
for ligne in desktop.splitlines():
    if ligne.startswith("StartupWMClass="):
        trouvee = ligne.split("=", 1)[1].strip()

if trouvee is None:
    echecs.append(f"{DESKTOP} : aucune ligne StartupWMClass")
elif trouvee != attendue:
    echecs.append(
        f"{DESKTOP} : StartupWMClass={trouvee} alors que la fenêtre émet "
        f"« {attendue} » ({raison})"
    )

# L'Exec du .desktop doit lancer le binaire que le manifeste déclare, sans quoi
# le raisonnement ci-dessus ne tient plus.
exec_ = None
for ligne in desktop.splitlines():
    if ligne.startswith("Exec="):
        exec_ = ligne.split("=", 1)[1].strip().split()[0]
if exec_ != commande:
    echecs.append(
        f"{DESKTOP} : Exec={exec_} ne correspond pas au `command: {commande}` "
        f"du manifeste"
    )

# L'AUR emballe le même exécutable : les deux .desktop doivent annoncer la même
# classe, c'est le moyen le plus simple de voir une divergence réapparaître.
aur = re.search(r"^StartupWMClass=(.+)$", pkgbuild, re.MULTILINE)
if aur is None:
    echecs.append(f"{PKGBUILD} : aucune ligne StartupWMClass dans le .desktop embarqué")
elif aur.group(1).strip() != attendue:
    echecs.append(
        f"{PKGBUILD} : StartupWMClass={aur.group(1).strip()} diverge de la "
        f"classe réelle « {attendue} » du même exécutable"
    )

# Le manifeste ne doit plus affirmer que GTK enregistre l'identifiant Tauri :
# c'est ce commentaire qui a fait écrire la mauvaise valeur.
manifeste_texte = open(MANIFESTE, encoding="utf-8").read()
if "GTK enregistre l'application sous l'identifiant de tauri.conf.json" in manifeste_texte:
    echecs.append(
        f"{MANIFESTE} : le commentaire affirme encore que GTK enregistre "
        f"l'identifiant de tauri.conf.json, ce qui est faux sans le drapeau"
    )

# L'autre moitié du même commentaire : `--own-name` n'est gardé que faute de
# preuve du contraire, et l'argument écrit est qu'aucun code du dépôt ne
# réclame de nom bien connu sur D-Bus (zbus n'arrive ici qu'en client du
# Secret Service, par keyring). Si un `request_name` apparaît un jour, la
# justification tombe : le contrôle force alors à rouvrir le dossier.
demandeurs = []
for racine in ("crates", "rdp-sidecar"):
    for source in pathlib.Path(racine).rglob("*.rs"):
        if "request_name" in source.read_text(encoding="utf-8", errors="replace"):
            demandeurs.append(str(source))
if demandeurs:
    echecs.append(
        "un code du dépôt réclame maintenant un nom D-Bus ("
        + ", ".join(sorted(demandeurs))
        + f") : relire le commentaire de --own-name dans {MANIFESTE}"
    )

if echecs:
    for e in echecs:
        print("  ✗", e, file=sys.stderr)
    sys.exit(1)

print(f"  ✓ StartupWMClass = « {attendue} » dans le .desktop Flathub et celui de l'AUR")
PY
