# Vérifie qu'aucun binaire Windows livré ne dépend du runtime Visual C++
# redistribuable (VCRUNTIME140*.dll, MSVCP140*.dll), absent d'un Windows neuf.
#
# Trouvé le 26 novembre 2026 en cherchant pourquoi une connexion RDP échouait
# depuis Windows : avash.exe lie ce runtime statiquement (tauri-build appelle
# static_vcruntime), mais avash-rdp.exe, construit hors de l'espace de travail,
# importait VCRUNTIME140.dll. Sur un poste sans le redistribuable, le processus
# RDP mourait au lancement (0xC0000135, STATUS_DLL_NOT_FOUND) et aucune
# connexion RDP ne pouvait aboutir. Les machines d'intégration ont ce runtime :
# le défaut ne s'y voyait pas à l'exécution, seulement en lisant les imports.
#
# Usage : scripts/verifier-runtime-windows.ps1 <binaire.exe> [<binaire.exe> ...]
param([Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)][string[]]$Binaires)
$ErrorActionPreference = "Stop"

$vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
$dumpbin = & $vswhere -latest -products * -find "VC\Tools\MSVC\**\bin\Hostx64\x64\dumpbin.exe" | Select-Object -First 1
if (-not $dumpbin) { throw "dumpbin introuvable : les outils MSVC sont-ils installés ?" }

$echecs = 0
foreach ($b in $Binaires) {
  if (-not (Test-Path $b)) { throw "binaire absent : $b" }
  $dependances = & $dumpbin /nologo /dependents $b | ForEach-Object { $_.Trim() } | Where-Object { $_ -match '\.dll$' }
  $redistribuable = $dependances | Where-Object { $_ -match '^(vcruntime|msvcp)\d' }
  if ($redistribuable) {
    Write-Output "✗ $b dépend du runtime Visual C++ redistribuable : $($redistribuable -join ', ')"
    $echecs++
  } else {
    Write-Output "✓ $b ne dépend que de Windows ($($dependances.Count) DLL)"
  }
}
if ($echecs -gt 0) { exit 1 }
