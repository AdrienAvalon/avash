#!/usr/bin/env bash
# Retire d'un répertoire `target` les unités que cargo n'utilise plus.
#
#   scripts/balayer-target.sh [--simuler] <target> <répertoire> <commande cargo> [<répertoire> <commande>...]
#
# Chaque paire rejoue, dans <répertoire> et avec CARGO_TARGET_DIR=<target>, la
# commande cargo donnée (par exemple « clippy --all-targets -- -D warnings »)
# en y ajoutant --message-format=json : à chaud, cargo ne recompile rien et
# énumère les artefacts de toutes les unités de son plan, fraîches comprises
# (compiler-artifact, build-script-executed). Les hachages de ces chemins sont
# les unités vivantes ; toute empreinte de <target>/<profil>/.fingerprint qui
# n'en porte aucun est périmée et part avec ses fichiers de deps/ et build/.
#
# Pourquoi cette voie et pas une autre (10 septembre 2026, target de 64 Go sur
# l'exécuteur GitLab, archive de cache de 19 Go, 21 minutes d'archivage pour 5
# de vérifications) : rien ne retire jamais un artefact d'un target, chaque
# montée de dépendance ou de version laisse ses anciens rlib et binaires de
# test ; l'heure d'accès des fichiers ne dit rien sur un disque monté noatime
# (celui du poste), et cargo stable ne réécrit invoked.timestamp que pour une
# unité recompilée. Seule l'énumération JSON dit ce qui sert.
#
# Garde-fou : une énumération qui ne rend aucun hachage (commande qui échoue,
# cargo muet) fait échouer le script sans rien retirer. Les commandes doivent
# être exactement celles du job (mêmes cibles, mêmes drapeaux après « -- ») :
# une unité oubliée n'est pas perdue, elle sera recompilée au passage suivant.
set -uo pipefail

simuler=0
if [ "${1:-}" = "--simuler" ]; then simuler=1; shift; fi
if [ $# -lt 3 ] || [ $(( ($# - 1) % 2 )) -ne 0 ]; then
  echo "usage : $0 [--simuler] <target> <répertoire> <commande cargo> [<répertoire> <commande>...]" >&2
  exit 2
fi
cible="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"; shift
[ -d "$cible" ] || { echo "balayage : $cible n'existe pas, rien à faire"; exit 0; }

vivants="$(mktemp)"
trap 'rm -f "$vivants"' EXIT

# Rejouer chaque commande ; le JSON va dans le fichier, le reste reste visible.
while [ $# -ge 2 ]; do
  repertoire="$1" commande="$2"; shift 2
  read -r sous_commande reste <<<"$commande"
  # shellcheck disable=SC2086 # les arguments de la commande sont volontairement éclatés
  if ! (cd "$repertoire" && CARGO_TARGET_DIR="$cible" cargo "$sous_commande" --message-format=json $reste >>"$vivants"); then
    echo "balayage : « cargo $commande » dans $repertoire a échoué, rien n'est retiré" >&2
    exit 1
  fi
done

SIMULER="$simuler" CIBLE="$cible" python3 - "$vivants" <<'PY'
import json, os, re, shutil, sys

cible = os.environ["CIBLE"]
simuler = os.environ["SIMULER"] == "1"
HACHAGE = re.compile(r"-([0-9a-f]{16})(?:\.[^/]*)?$")

def hachage(nom):
    m = HACHAGE.search(nom)
    return m.group(1) if m else None

# 1. Les hachages vivants : tout composant de chemin sous <target> qui se
#    termine par -<16 hex> (deps/libx-h.rlib, build/x-h/out, build/x-h/...).
vivants = set()
with open(sys.argv[1], encoding="utf-8") as f:
    for ligne in f:
        ligne = ligne.strip()
        if not ligne.startswith("{"):
            continue
        try:
            m = json.loads(ligne)
        except json.JSONDecodeError:
            continue
        chemins = list(m.get("filenames") or [])
        if m.get("out_dir"):
            chemins.append(m["out_dir"])
        for chemin in chemins:
            for composant in chemin.split("/"):
                h = hachage(composant)
                if h:
                    vivants.add(h)
if not vivants:
    print("balayage : aucune unité énumérée, rien n'est retiré", file=sys.stderr)
    sys.exit(1)

# 2. Chaque profil (debug, release, ...) directement sous <target>.
def taille(chemin):
    if os.path.isfile(chemin):
        return os.path.getsize(chemin)
    total = 0
    for racine, _, fichiers in os.walk(chemin):
        for f in fichiers:
            try:
                total += os.path.getsize(os.path.join(racine, f))
            except OSError:
                pass
    return total

retirees, octets, gardees = 0, 0, 0
for profil in sorted(os.listdir(cible)):
    empreintes = os.path.join(cible, profil, ".fingerprint")
    if not os.path.isdir(empreintes):
        continue
    for unite in sorted(os.listdir(empreintes)):
        h = hachage(unite)
        if h is None:
            continue
        if h in vivants:
            gardees += 1
            continue
        victimes = [os.path.join(empreintes, unite)]
        for sous in ("deps", "build", "incremental", "examples"):
            dossier = os.path.join(cible, profil, sous)
            if not os.path.isdir(dossier):
                continue
            for nom in os.listdir(dossier):
                if hachage(nom) == h:
                    victimes.append(os.path.join(dossier, nom))
        poids = sum(taille(v) for v in victimes)
        retirees += 1
        octets += poids
        if simuler:
            print(f"  périmée : {profil}/{unite} ({poids / 1e6:.0f} Mo, {len(victimes)} entrées)")
            continue
        for v in victimes:
            if os.path.isdir(v) and not os.path.islink(v):
                shutil.rmtree(v, ignore_errors=True)
            else:
                try:
                    os.remove(v)
                except FileNotFoundError:
                    pass

verbe = "à retirer" if simuler else "retirées"
print(f"balayage de {cible} : {retirees} unités {verbe}, {octets / 1e9:.2f} Go, {gardees} unités vivantes")
PY
