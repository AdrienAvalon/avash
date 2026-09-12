#!/usr/bin/env bash
# Contrôle reproductible : les sources figées de Flathub (packaging/flathub/
# *.json) disent la même chose que les verrous du dépôt, et le générateur qui
# les écrit est épinglé sur un commit.
#
# Trouvé par l'audit du 12 septembre 2026 (C-chaine-9) : scripts/flathub-
# sources.sh clonait flatpak-builder-tools sur sa branche principale, sans
# commit épinglé, puis exécutait son générateur. Sa sortie (l'URL et
# l'empreinte de chaque source que Flathub téléchargera) est commitée : un
# commit amont malveillant pouvait y réécrire une empreinte au milieu de
# plusieurs milliers de lignes, invisible en revue.
#
# Deux vérifications :
#   1. le script épingle un commit de 40 caractères et refuse d'en exécuter un
#      autre ;
#   2. chaque archive de crate a l'empreinte que donne le Cargo.lock
#      correspondant (et son .cargo-checksum.json la même), chaque paquet npm
#      celle de l'`integrity` de web/package-lock.json, et tout vient des deux
#      registres officiels. Une entrée que le verrou ne connaît plus est
#      périmée, pas fausse : elle n'échoue qu'en mode STRICT=1, celui que
#      flathub-sources.sh impose juste après avoir régénéré (une montée de
#      dépendance ne doit pas rougir la porte avant la régénération prévue par
#      RELEASE.md).
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

echecs=0
script="scripts/flathub-sources.sh"
if ! grep -qE '^COMMIT_OUTILS="?[0-9a-f]{40}"?$' "$script"; then
  echo "  ✗ $script : aucun commit de flatpak-builder-tools épinglé (COMMIT_OUTILS=<40 hex>)" >&2
  echecs=1
fi
if ! grep -q 'rev-parse HEAD' "$script"; then
  echo "  ✗ $script : le commit du générateur n'est pas vérifié avant exécution" >&2
  echecs=1
fi
if grep -qE 'git clone --depth 1 https://github.com/flatpak/flatpak-builder-tools' "$script"; then
  echo "  ✗ $script : clone de la branche principale, sans commit" >&2
  echecs=1
fi

STRICT="${STRICT:-0}" python3 - <<'PY' || echecs=1
import base64
import json
import os
import re
import sys
import tomllib

strict = os.environ.get("STRICT") == "1"
echecs, perimees = [], []

CRATE = re.compile(r"^https://static\.crates\.io/crates/([^/]+)/([^/]+)\.crate$")
for sources, verrou in (("packaging/flathub/cargo-sources.json", "Cargo.lock"),
                        ("packaging/flathub/cargo-sources-rdp.json", "rdp-sidecar/Cargo.lock")):
    lock = tomllib.load(open(verrou, "rb"))
    attendu = {(p["name"], p["version"]): p["checksum"]
               for p in lock.get("package", []) if p.get("checksum")}
    entrees = json.load(open(sources, encoding="utf-8"))
    par_dest, vus = {}, set()
    for e in entrees:
        if e.get("type") != "archive":
            continue
        m = CRATE.match(e.get("url", ""))
        if not m:
            echecs.append(f"{sources} : source hors de static.crates.io : {e.get('url')}")
            continue
        nom, fichier = m.group(1), m.group(2)
        version = fichier[len(nom) + 1:] if fichier.startswith(nom + "-") else None
        cle = (nom, version)
        par_dest[e.get("dest")] = e.get("sha256")
        if cle in attendu:
            vus.add(cle)
            if e.get("sha256") != attendu[cle]:
                echecs.append(f"{sources} : {nom} {version} a l'empreinte {e.get('sha256')}, le verrou dit {attendu[cle]}")
        else:
            perimees.append(f"{sources} : {nom} {version} absent de {verrou}")
    for e in entrees:
        if e.get("type") == "inline" and e.get("dest-filename") == ".cargo-checksum.json":
            paquet = json.loads(e["contents"]).get("package")
            if par_dest.get(e.get("dest")) not in (None, paquet):
                echecs.append(f"{sources} : .cargo-checksum.json de {e.get('dest')} ne reprend pas l'empreinte de l'archive")
    if strict:
        for cle in sorted(set(attendu) - vus):
            echecs.append(f"{sources} : {cle[0]} {cle[1]} du verrou manque")

lock = json.load(open("web/package-lock.json", encoding="utf-8"))
attendu = {}
for p in lock.get("packages", {}).values():
    if p.get("resolved") and p.get("integrity"):
        algo, _, b64 = p["integrity"].split()[0].partition("-")
        attendu[p["resolved"]] = (algo, base64.b64decode(b64).hex())
vus = set()
for e in json.load(open("packaging/flathub/node-sources.json", encoding="utf-8")):
    if e.get("type") != "file":
        continue
    url = e.get("url", "")
    if not url.startswith("https://registry.npmjs.org/"):
        echecs.append(f"node-sources.json : source hors du registre npm : {url}")
        continue
    if url not in attendu:
        perimees.append(f"node-sources.json : {url} absent de web/package-lock.json")
        continue
    vus.add(url)
    algo, empreinte = attendu[url]
    if e.get(algo) != empreinte:
        echecs.append(f"node-sources.json : {url} n'a pas l'empreinte {algo} du verrou")
if strict:
    for url in sorted(set(attendu) - vus):
        echecs.append(f"node-sources.json : {url} du verrou manque")

if strict:
    echecs += perimees
if echecs:
    for e in echecs[:40]:
        print(f"  ✗ {e}", file=sys.stderr)
    if len(echecs) > 40:
        print(f"  ✗ … et {len(echecs) - 40} autres", file=sys.stderr)
    sys.exit(1)
suffixe = f" ({len(perimees)} entrées périmées : relancer scripts/flathub-sources.sh avant la prochaine soumission)" if perimees else ""
print(f"  ✓ sources Flathub conformes aux verrous{suffixe}")
PY

exit "$echecs"
