<#
.SYNOPSIS
  Régénère portcls-sys\tests\layout.golden : sizeof et GUID de référence calculés par cl.exe.
.DESCRIPTION
  Compile portcls-sys\tools\sizeof-probe.c avec cl.exe (Build Tools, trouvés par vswhere)
  sur les en-têtes km et shared du WDK (version de packaging\windows\versions.json), avec
  les mêmes défines que wrapper.h (_AMD64_, AMD64, _WIN64, _KERNEL_MODE), en édition de
  liens utilisateur (CRT statique). Exécute le programme et écrit sa sortie (une mesure
  par ligne, LF, UTF-8 sans BOM) dans le fichier golden lu par tests\layout.rs.
  Deux compilateurs sur les mêmes en-têtes (clang via bindgen, cl.exe ici) : c'est la
  vérification demandée par M1a-03. Voir docs/driver-dev.md §2.
.PARAMETER OutFile
  Fichier de sortie (défaut : portcls-sys\tests\layout.golden). check.ps1 l'écrit dans un
  dossier temporaire pour le comparer au golden commité.
#>
param([string]$OutFile)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$workspace = Split-Path -Parent $PSScriptRoot
$repo = Split-Path -Parent (Split-Path -Parent $workspace)
$source = Join-Path $workspace "portcls-sys\tools\sizeof-probe.c"
if (-not $OutFile) { $OutFile = Join-Path $workspace "portcls-sys\tests\layout.golden" }

function Fail([string]$message) {
  $Host.UI.WriteErrorLine("regen-layout : $message")
  exit 1
}

# Versions épinglées (même source que setup-env.ps1).
$versions = Get-Content (Join-Path $repo "packaging\windows\versions.json") -Raw | ConvertFrom-Json
$wdkVersion = $versions.wdk

# cl.exe des Build Tools, par vswhere (comme setup-env.ps1 pour link.exe).
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (-not (Test-Path $vswhere)) { Fail "vswhere introuvable ($vswhere) : Build Tools absents ?" }
$cl = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find "VC\Tools\MSVC\*\bin\Hostx64\x64\cl.exe" | Select-Object -First 1
if (-not $cl) { Fail "cl.exe (MSVC x64) introuvable via vswhere" }
# ...\VC\Tools\MSVC\<version>\bin\Hostx64\x64\cl.exe -> ...\VC\Tools\MSVC\<version>\lib\x64
$msvcRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $cl)))
$msvcLib = Join-Path $msvcRoot "lib\x64"
if (-not (Test-Path (Join-Path $msvcLib "libcmt.lib"))) { Fail "libcmt.lib absent de $msvcLib" }

# Racine du WDK : WDKContentRoot puis le registre (là où wdk-build la cherche).
$kitsRoot = $env:WDKContentRoot
if (-not $kitsRoot) {
  $props = Get-ItemProperty -Path "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots" -ErrorAction SilentlyContinue
  if ($null -ne $props -and ($props.PSObject.Properties.Name -contains "KitsRoot10")) { $kitsRoot = $props.KitsRoot10 }
}
if (-not $kitsRoot) { Fail "WDK introuvable (ni WDKContentRoot ni KitsRoot10)" }
$include = Join-Path $kitsRoot "Include\$wdkVersion"
$lib = Join-Path $kitsRoot "Lib\$wdkVersion"
foreach ($required in @("$include\km\portcls.h", "$include\shared\ks.h", "$lib\um\x64\kernel32.lib", "$lib\ucrt\x64\libucrt.lib")) {
  if (-not (Test-Path $required)) { Fail "WDK/SDK $wdkVersion incomplet : $required manquant" }
}

$build = Join-Path ([IO.Path]::GetTempPath()) ("conduit-sizeof-probe-" + [IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Force $build | Out-Null
try {
  $exe = Join-Path $build "sizeof-probe.exe"
  $args = @(
    "/nologo", "/W4", "/MT",
    "/D_AMD64_", "/DAMD64", "/D_WIN64", "/D_KERNEL_MODE",
    "/I$include\km\crt", "/I$include\km", "/I$include\shared",
    "/Fo$build\", "/Fe$exe",
    $source,
    "/link", "/LIBPATH:$msvcLib", "/LIBPATH:$lib\ucrt\x64", "/LIBPATH:$lib\um\x64"
  )
  Write-Host "== cl.exe $(Split-Path -Leaf $source)"
  & $cl @args
  if ($LASTEXITCODE -ne 0) { Fail "compilation de sizeof-probe.c échouée ($LASTEXITCODE)" }

  $lines = & $exe
  if ($LASTEXITCODE -ne 0) { Fail "sizeof-probe.exe a renvoyé $LASTEXITCODE" }
  if (@($lines).Count -lt 10) { Fail "sortie de sizeof-probe.exe trop courte" }

  $header = @(
    "# Généré par drivers/windows/tools/regen-layout.ps1 (cl.exe, WDK $wdkVersion, x64) : ne pas éditer.",
    "# sizeof<TAB>Nom<TAB>octets | guid<TAB>Nom<TAB>XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX"
  )
  $text = (($header + @($lines)) -join "`n") + "`n"
  $outDir = Split-Path -Parent $OutFile
  if ($outDir -and -not (Test-Path $outDir)) { New-Item -ItemType Directory -Force $outDir | Out-Null }
  [IO.File]::WriteAllText($OutFile, $text, (New-Object System.Text.UTF8Encoding $false))
  Write-Host "golden écrit : $OutFile ($(@($lines).Count) mesures)"
} finally {
  Remove-Item -Recurse -Force $build -ErrorAction SilentlyContinue
}
exit 0
