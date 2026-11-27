#!/usr/bin/env bash
# Contrôle reproductible : ce qui part chez les utilisateurs est construit sans
# aucun cache d'Actions, avec un tauri-cli installé à part, sans la
# fonctionnalité `webdriver`, et le binaire est vérifié avant l'envoi.
#
#   scripts/tests/release-construit-sans-cache.sh [racine du dépôt]
#
# Trouvé par l'audit du 27 novembre 2026 (SE-1) : le job `build` de
# release.yml restaurait par Swatinem/rust-cache `./target`,
# `rdp-sidecar/target` et `~/.cargo` (binaires installés compris). La clé de
# rust-cache est faite de l'identifiant du job, pas du nom du workflow : le job
# `build` de ci.yml, sur la même image, écrivait à chaque poussée sur main
# l'entrée que la publication Linux restaurait par sa clé de repli. Ce job-là
# compile les serveurs de test, cargo-audit et cargo-deny depuis leurs sources,
# lance ESLint, knip et vitest : n'importe lequel de ces codes atteignait le
# binaire signé (des `.rlib` frais liés tels quels, un outil déposé dans
# `~/.cargo/bin` que `cargo install` ne réinstalle pas). Et tout code d'un job
# de main peut écrire une entrée sous n'importe quelle clé (« Cacheract »).
# SE-2, même audit : la règle « jamais `webdriver` dans un binaire publié » ne
# tenait qu'à l'absence d'une option dans une commande.
#
# Jobs gardés : chaque job dont `signer` ou `publier` de release.yml dépendent
# (directement ou non), et tout job d'un workflow qui assemble des bundles
# (`tauri build`, essai-windows.yml compris : son installeur va sur de vraies
# machines). Seule exception, nommée : `securite` de release.yml, qui joue le
# fuzz sur son cache et ne produit rien de livré ; on vérifie qu'il n'envoie
# aucun artefact, sans quoi l'exception n'aurait plus d'objet.
#
# Règles, par job gardé :
#   1. aucune action de cache (Swatinem/rust-cache, actions/cache et ses
#      sous-actions), aucun paramètre `cache` ou `cache-*` à une action ;
#   2. setup-node pose `package-manager-cache: false` (sinon il met lui-même
#      npm en cache dès qu'un package.json déclare `packageManager`) ;
#   3. `cargo install` installe sous `$RUNNER_TEMP` (`--root`), jamais dans
#      `~/.cargo/bin` ;
#   4. tauri-cli n'est jamais appelé en sous-commande (`cargo tauri`, qui prend
#      le premier `cargo-tauri` trouvé) mais par le chemin de cette racine ;
#   5. aucune commande cargo ni tauri ne pose `--features`, `-F`, `-f` ou
#      `--all-features` ;
#   6. un job qui assemble des bundles vérifie son binaire par
#      scripts/verifier-sans-webdriver.sh avant d'envoyer ses artefacts ;
# et, dans ci.yml, chaque job qui construit avec `--features webdriver` joue le
# contrôle inverse (`--doit-contenir`).
set -euo pipefail
RACINE="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
cd "$RACINE"

python3 - <<'PY'
import glob
import re
import sys

import yaml

EXCEPTIONS = {("release.yml", "securite")}
TAURI_SOUS_COMMANDE = re.compile(r"\bcargo\s+tauri\b")
TAURI_BUILD = re.compile(r"\bcargo(?:\s+|-)tauri\"?\s+build\b")
CARGO = re.compile(r"\bcargo(?:\s+\+\S+)?\s+(?:build|test|check|clippy|run|install)\b")
FONCTIONNALITES = re.compile(r"(?:^|\s)(?:--features|--all-features|-F|-f)(?:[\s=]|$)")
ACTIONS_CACHE = ("Swatinem/rust-cache", "actions/cache")

echecs = []
gardes = []


def charger(nom):
    try:
        return yaml.safe_load(open(f".github/workflows/{nom}", encoding="utf-8"))
    except FileNotFoundError:
        return None


def lignes(run):
    tampon = ""
    for ligne in str(run or "").splitlines():
        if ligne.rstrip().endswith("\\"):
            tampon += ligne.rstrip()[:-1] + " "
            continue
        yield tampon + ligne
        tampon = ""


def code(ligne):
    nue = ligne.strip()
    return "" if nue.startswith("#") else ligne


def assemble(job):
    return any(TAURI_BUILD.search(code(l)) for e in job.get("steps") or [] for l in lignes(e.get("run")))


def dependances(jobs, depart):
    vus, pile = set(), list(depart)
    while pile:
        nom = pile.pop()
        if nom in vus or nom not in jobs:
            continue
        vus.add(nom)
        besoins = jobs[nom].get("needs") or []
        pile.extend([besoins] if isinstance(besoins, str) else besoins)
    return vus


a_garder = set()
release = charger("release.yml")
if release is None:
    echecs.append("release.yml absent")
else:
    jobs = release.get("jobs") or {}
    for final in ("signer", "publier"):
        if final not in jobs:
            echecs.append(f"release.yml : job `{final}` absent, la chaîne de publication n'est plus celle que cette garde connaît")
    for nom in dependances(jobs, ["signer", "publier"]) - {"signer", "publier"}:
        a_garder.add(("release.yml", nom))

for chemin in sorted(glob.glob(".github/workflows/*.yml")):
    nom_wf = chemin.split("/")[-1]
    wf = yaml.safe_load(open(chemin, encoding="utf-8"))
    for nom_job, job in (wf.get("jobs") or {}).items():
        if assemble(job):
            a_garder.add((nom_wf, nom_job))

for nom_wf, nom_job in sorted(a_garder):
    court = f"{nom_wf}:{nom_job}"
    job = (charger(nom_wf).get("jobs") or {})[nom_job]
    etapes = job.get("steps") or []
    if (nom_wf, nom_job) in EXCEPTIONS:
        for e in etapes:
            if str(e.get("uses", "")).startswith("actions/upload-artifact"):
                echecs.append(f"{court} : job exempté du « sans cache » parce qu'il ne livre rien, mais il envoie un artefact")
        continue
    gardes.append(court)
    racines = set()
    verifie_binaire = False
    for i, e in enumerate(etapes):
        uses = str(e.get("uses", ""))
        nom = e.get("name") or uses.split("@")[0] or "?"
        avec = e.get("with") or {}
        if uses.startswith(ACTIONS_CACHE):
            echecs.append(f"{court} : l'étape « {nom} » restaure un cache ({uses.split('@')[0]})")
        for cle in avec:
            if cle == "cache" or str(cle).startswith("cache-"):
                echecs.append(f"{court} : l'étape « {nom} » passe `{cle}` à {uses.split('@')[0]}")
        if uses.startswith("actions/setup-node") and avec.get("package-manager-cache") is not False:
            echecs.append(f"{court} : setup-node sans `package-manager-cache: false` (cache npm automatique)")
        for ligne in lignes(e.get("run")):
            c = code(ligne)
            if not c:
                continue
            if "verifier-sans-webdriver.sh" in c and "--doit-contenir" not in c:
                verifie_binaire = True
            if re.search(r"\bcargo\s+install\b", c):
                m = re.search(r"--root\s+\"?(\$RUNNER_TEMP|\$\{\{\s*runner\.temp\s*\}\})/([^\s\"]+)", c)
                if not m:
                    echecs.append(f"{court} : « {c.strip()[:90]} » installe dans ~/.cargo/bin (pas de --root sous $RUNNER_TEMP)")
                else:
                    racines.add(m.group(2))
            if TAURI_SOUS_COMMANDE.search(c):
                echecs.append(f"{court} : « {c.strip()[:90]} » appelle `cargo tauri` (premier cargo-tauri trouvé) au lieu de l'outil installé à part")
            elif TAURI_BUILD.search(c):
                m = re.search(r"(\$RUNNER_TEMP|\$\{\{\s*runner\.temp\s*\}\})/([^\s\"]+)/bin/cargo-tauri\"?\s+build", c)
                if not m or m.group(2) not in racines:
                    echecs.append(f"{court} : `cargo-tauri build` n'est pas appelé par le chemin de la racine où le job l'a installé")
            if (CARGO.search(c) or TAURI_BUILD.search(c)) and FONCTIONNALITES.search(c):
                echecs.append(f"{court} : « {c.strip()[:90]} » pose des fonctionnalités cargo dans un job qui livre")
        if uses.startswith("actions/upload-artifact") and assemble(job) and not verifie_binaire:
            echecs.append(f"{court} : artefacts envoyés (« {nom} ») avant toute vérification par scripts/verifier-sans-webdriver.sh")
    if assemble(job) and not verifie_binaire:
        echecs.append(f"{court} : assemble des bundles sans vérifier son binaire par scripts/verifier-sans-webdriver.sh")

# Contrôle inverse : tout binaire construit avec --features webdriver dans
# ci.yml passe par la garde en --doit-contenir.
ci = charger("ci.yml") or {}
inverses = []
for nom_job, job in (ci.get("jobs") or {}).items():
    construit, controle = False, False
    for e in job.get("steps") or []:
        for ligne in lignes(e.get("run")):
            c = code(ligne)
            if re.search(r"--features\s+webdriver\b", c) and CARGO.search(c):
                construit = True
            if "verifier-sans-webdriver.sh" in c and "--doit-contenir" in c:
                if not construit:
                    echecs.append(f"ci.yml:{nom_job} : contrôle inverse joué avant la construction webdriver")
                controle = True
    if construit:
        inverses.append(nom_job)
        if not controle:
            echecs.append(f"ci.yml:{nom_job} : construit avec --features webdriver sans le contrôle inverse (verifier-sans-webdriver.sh --doit-contenir)")
if not inverses:
    echecs.append("ci.yml : aucun job ne construit avec --features webdriver, le contrôle inverse ne tourne nulle part")

if echecs:
    for e in echecs:
        print(f"  ✗ {e}", file=sys.stderr)
    sys.exit(1)
print(f"  ✓ sans cache, outil Tauri à part, sans webdriver : {', '.join(gardes)} ; contrôle inverse dans ci.yml:{', ci.yml:'.join(inverses)}")
PY
