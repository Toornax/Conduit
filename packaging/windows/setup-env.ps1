<#
.SYNOPSIS
  Installe ou vérifie l'environnement de build Windows épinglé (SPEC §5.11, M0-08).
.PARAMETER Check
  Ne rien installer : échouer si une version diffère de versions.json.
#>
param([switch]$Check)

$ErrorActionPreference = "Stop"
$versions = Get-Content (Join-Path $PSScriptRoot "versions.json") | ConvertFrom-Json
$toolchain = "$($versions.rust)-x86_64-pc-windows-msvc"

function Fail($msg) { Write-Error $msg; exit 1 }

if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
  if ($Check) { Fail "rustup absent : installez-le depuis https://rustup.rs" }
  Invoke-WebRequest https://win.rustup.rs/x86_64 -OutFile "$env:TEMP\rustup-init.exe"
  & "$env:TEMP\rustup-init.exe" -y --default-toolchain $toolchain --profile minimal
  $env:Path += ";$env:USERPROFILE\.cargo\bin"
}

$installed = (& rustup toolchain list) -join "`n"
if ($installed -notmatch [regex]::Escape($toolchain)) {
  if ($Check) { Fail "toolchain $toolchain absente (installées : $installed)" }
  & rustup toolchain install $toolchain --profile minimal
}
foreach ($c in $versions.components) {
  $has = (& rustup component list --toolchain $toolchain --installed) -join "`n"
  if ($has -notmatch "^$c") {
    if ($Check) { Fail "composant $c absent" }
    & rustup component add $c --toolchain $toolchain
  }
}
$rustc = (& rustup run $toolchain rustc --version)
if ($rustc -notmatch [regex]::Escape($versions.rust)) { Fail "rustc inattendu : $rustc (attendu $($versions.rust))" }

# Visual Studio Build Tools : présence du compilateur MSVC (version majeure épinglée).
$vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (Test-Path $vswhere) {
  $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationVersion
  if (-not $vs) { Fail "Visual Studio Build Tools avec les outils C++ introuvables" }
  if ($vs.Split(".")[0] -ne $versions.visual_studio_build_tools) { Fail "Build Tools $vs, attendu $($versions.visual_studio_build_tools).x" }
} elseif ($Check) {
  Fail "vswhere introuvable : installez Visual Studio Build Tools $($versions.visual_studio_build_tools)"
}

Write-Host "environnement Windows conforme : rust $($versions.rust), Build Tools $($versions.visual_studio_build_tools)"
