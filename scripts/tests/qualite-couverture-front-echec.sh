#!/usr/bin/env bash
# Contrôle reproductible de l'étape « Couverture du front (Vitest) » du workflow
# .github/workflows/qualite.yml.
#
# Trouvé par l'audit du 7 septembre 2026 : l'étape n'avait pas de `shell:`
# explicite. GitHub lance alors `bash -e {0}` SANS `-o pipefail` (pipefail n'est
# posé que lorsque `shell: bash` est écrit). Le `run` finissait par
# `npx vitest run --coverage … | tee ../couverture/front.txt` : le code de
# retour du tube est celui de `tee`, toujours 0. Un Vitest en échec (test qui
# régresse, config de couverture cassée, @vitest/coverage-v8 manquant, aucun
# test trouvé) laissait l'étape verte et le chiffre de couverture front absent
# ou faux dans le résumé hebdomadaire relu à la main.
#
# Ce test reconstitue la règle de sélection du shell de GitHub (shell absent →
# `bash -e`, pas de pipefail ; `shell: bash` → `bash -eo pipefail`), remplace
# `npx` par un faux qui simule un Vitest qui échoue, rejoue le `run` réel de
# l'étape et exige que l'échec remonte (code de retour non nul). Contre le
# workflow masqué il échoue ; avec `shell: bash` ou une redirection sans tube
# il passe.
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

WORKFLOW=".github/workflows/qualite.yml"

python3 - "$WORKFLOW" <<'PY'
import os, subprocess, sys, tempfile, yaml

workflow = yaml.safe_load(open(sys.argv[1], encoding="utf-8"))
etapes = workflow["jobs"]["couverture"]["steps"]
etape = next(
    e for e in etapes
    if e.get("name", "").startswith("Couverture du front")
)
run = etape["run"]

# Sélection du shell, telle que le runner GitHub la fait pour un `run` :
#   - pas de clé `shell:` → shell par défaut `bash -e {0}`, SANS pipefail ;
#   - `shell: bash`       → `bash --noprofile --norc -eo pipefail {0}`.
# C'est toute la différence : un tube ne masque le code de retour que dans le
# premier cas. `set -o pipefail` posé dans le corps du `run` relève aussi la
# garde, et le rejeu ci-dessous le prendrait naturellement en compte.
shell = etape.get("shell")
if shell is None:
    flags = ["-e"]
elif shell == "bash":
    flags = ["-e", "-o", "pipefail"]
else:
    print(f"  ~ shell '{shell}' non modélisé, contrôle sauté", file=sys.stderr)
    sys.exit(0)

bac = tempfile.mkdtemp(prefix="avash-qualite-")
# L'étape a `working-directory: web` et écrit dans `../couverture/…` : on
# reproduit cette arborescence pour que la redirection ou le `tee` aboutisse.
web = os.path.join(bac, "web")
couverture = os.path.join(bac, "couverture")
bind = os.path.join(bac, "bin")
for d in (web, couverture, bind):
    os.makedirs(d, exist_ok=True)

# Faux `npx` : simule un Vitest qui échoue (test régressé, crash au démarrage).
# Il écrit sur stdout comme le ferait le rapporteur « text », puis sort non nul.
faux_npx = os.path.join(bind, "npx")
with open(faux_npx, "w", encoding="utf-8") as f:
    f.write(
        "#!/usr/bin/env bash\n"
        "echo 'FAIL web/exemple.test.ts > un test régressé'\n"
        "exit 1\n"
    )
os.chmod(faux_npx, 0o755)

script = os.path.join(bac, "etape.sh")
with open(script, "w", encoding="utf-8") as f:
    f.write(run)

env = dict(os.environ, PATH=bind + os.pathsep + os.environ["PATH"])
code = subprocess.run(["bash", *flags, script], cwd=web, env=env).returncode

if code == 0:
    print(
        "  ✗ un Vitest en échec laisse l'étape « Couverture du front » verte "
        "(code de retour masqué par le tube) : une régression du front n'est "
        "signalée par aucun job Qualité",
        file=sys.stderr,
    )
    sys.exit(1)

print("  ✓ un Vitest en échec fait rougir l'étape « Couverture du front »")
PY
