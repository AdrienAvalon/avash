#!/usr/bin/env bash
# Contrôle reproductible : scripts/npm-audit.sh ne prend jamais un avis de
# sécurité pour une panne du registre.
#
# Trouvé par l'audit du 12 septembre 2026 (C-chaine-11) : la décision « registre
# en panne » reposait sur un grep (ECONNRESET, socket hang up, Service
# Unavailable…) appliqué à toute la sortie de `npm audit`, titres d'avis
# compris. Un avis dont le titre contenait l'un de ces mots était réessayé
# trois fois puis, en mode tolerer-registre (suite bout en bout), toléré :
# exit 0 sur une vulnérabilité critique. La décision se prend désormais sur la
# STRUCTURE de `npm audit --json` : un rapport (auditReportVersion) n'est jamais
# une panne, quel que soit son texte ; une panne est un objet `error` sans
# rapport, dont le code est absent (point d'audit injoignable, forme vérifiée
# avec npm 12) ou réseau.
#
# Le vrai script est rejoué avec un `npm` simulé, sans attendre entre les essais
# (NPM_AUDIT_PAUSE=0).
set -euo pipefail
cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

bac="$(mktemp -d)"
trap 'rm -rf "$bac"' EXIT
mkdir -p "$bac/stub"
cat > "$bac/stub/npm" <<'STUB'
#!/usr/bin/env bash
echo x >> "$STUB_COMPTE"
rapport() { # <critical> <high> <titre>
  cat <<JSON
{"auditReportVersion": 2,
 "vulnerabilities": {"paquet": {"name": "paquet", "severity": "$( [ "$1" -gt 0 ] && echo critical || echo high )",
   "via": [{"title": "$3", "severity": "critical"}]}},
 "metadata": {"vulnerabilities": {"info": 0, "low": 0, "moderate": 0, "high": $2, "critical": $1, "total": $(( $1 + $2 ))}}}
JSON
}
# Comme le vrai npm : code 1 dès qu'un avis atteint --audit-level.
seuil="$(printf '%s\n' "$@" | sed -n 's/^--audit-level=//p')"
code_pour() { # <critical> <high>
  if [ "$1" -gt 0 ] || { [ "$2" -gt 0 ] && [ "$seuil" != critical ]; }; then echo 1; else echo 0; fi
}
case "$STUB_SCENARIO" in
  avis-piege)  rapport 1 0 "ECONNRESET: socket hang up, Service Unavailable"; exit "$(code_pour 1 0)" ;;
  panne)       echo '{"message": "request to https://registry.npmjs.org/-/npm/v1/security/advisories/bulk failed, reason: socket hang up", "error": {"summary": "", "detail": ""}}'; exit 1 ;;
  propre)      rapport 0 0 ""; exit 0 ;;
  sans-verrou) echo '{"error": {"code": "ENOLOCK", "summary": "This command requires an existing lockfile.", "detail": ""}}'; exit 1 ;;
  haute)       rapport 0 2 "prototype pollution"; exit "$(code_pour 0 2)" ;;
  incoherent)  rapport 0 0 ""; exit 1 ;;
esac
STUB
chmod +x "$bac/stub/npm"

echecs=0
jouer() { # <scénario> <code attendu : 0 ou non-zéro> <appels attendus> <args…>
  local scenario="$1" attendu="$2" appels="$3"; shift 3
  : > "$bac/compte"
  local code=0
  env PATH="$bac/stub:$PATH" STUB_SCENARIO="$scenario" STUB_COMPTE="$bac/compte" NPM_AUDIT_PAUSE=0 \
    bash scripts/npm-audit.sh "$@" >/dev/null 2>&1 || code=$?
  local n; n="$(wc -l < "$bac/compte")"
  if { [ "$attendu" = 0 ] && [ "$code" -ne 0 ]; } || { [ "$attendu" != 0 ] && [ "$code" -eq 0 ]; }; then
    echo "  ✗ $scenario ($*) : code $code, attendu $( [ "$attendu" = 0 ] && echo 0 || echo 'non nul')" >&2; echecs=1
  fi
  if [ "$n" -ne "$appels" ]; then
    echo "  ✗ $scenario ($*) : $n appel(s) à npm audit, attendu $appels" >&2; echecs=1
  fi
}

jouer avis-piege  1 1 critical tolerer-registre   # l'avis piège n'est ni réessayé ni toléré
jouer avis-piege  1 1 high
jouer panne       0 3 critical tolerer-registre   # vraie panne, tolérée après trois essais
jouer panne       1 3 high                        # vraie panne, front : pas de verdict, échec
jouer propre      0 1 high
jouer sans-verrou 1 1 critical tolerer-registre   # une erreur locale n'est pas une panne du registre
jouer haute       0 1 critical                    # sous le seuil demandé
jouer haute       1 1 high                        # au seuil
jouer incoherent  1 1 high                        # npm échoue sans rapport ni panne lisible

if [ "$echecs" -ne 0 ]; then
  exit 1
fi
echo "  ✓ npm-audit.sh décide sur la structure de npm audit --json : un avis n'est jamais pris pour une panne"
