<#
.SYNOPSIS
  Installe ou vérifie l'environnement de build Windows épinglé (SPEC §5.11, M0-08, M1a-01).
.DESCRIPTION
  Les versions attendues sont dans versions.json. Sans -Check, le script installe ce qui
  ne demande pas d'élévation (rustup, toolchain, composants, cargo-wdk) et échoue avec la
  commande winget à lancer en administrateur pour le reste (Build Tools, WDK, LLVM).
  Voir docs/driver-dev.md.
.PARAMETER Check
  Ne rien installer : échouer si un outil manque ou si une version diffère de versions.json.
#>
param([switch]$Check)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$versions = Get-Content (Join-Path $PSScriptRoot "versions.json") -Raw | ConvertFrom-Json
$toolchain = "$($versions.rust)-x86_64-pc-windows-msvc"
$kitsRootKey = "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots"

# Commandes d'installation à lancer dans un PowerShell administrateur (le script ne
# s'élève jamais lui-même).
$wingetBuildTools = 'winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --add Microsoft.VisualStudio.Component.Windows11SDK.26100 --add Microsoft.VisualStudio.Component.VC.Runtimes.x86.x64.Spectre --includeRecommended"'
$wingetWdk = "winget install --id Microsoft.WindowsWDK.10.0.26100 -e"
$wingetLlvm = "winget install --id LLVM.LLVM -e --version $($versions.llvm) --force"

function Fail([string]$Message) {
  # Pas de Write-Error : avec ErrorActionPreference = Stop, il interromprait le script avant
  # `exit 1` et le code de sortie dépendrait de la façon dont le script a été lancé.
  $Host.UI.WriteErrorLine("setup-env : $Message")
  exit 1
}

function Require-Admin-Install([string]$What, [string]$Command) {
  Fail "$What : lancez dans un PowerShell administrateur puis relancez ce script :`n  $Command"
}

# Ajoute ~/.cargo/bin au PATH de la session si rustup vient d'être installé ou si le PATH
# n'a pas encore été rechargé.
function Add-CargoBinToPath {
  $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
  if (($env:Path -split ";") -notcontains $cargoBin) { $env:Path = "$cargoBin;$env:Path" }
}

function Require-Rust {
  if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    Add-CargoBinToPath
  }
  if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
    if ($Check) { Fail "rustup absent : installez-le depuis https://rustup.rs" }
    $init = Join-Path $env:TEMP "rustup-init.exe"
    Invoke-WebRequest https://win.rustup.rs/x86_64 -OutFile $init
    & $init -y --default-toolchain $toolchain --profile minimal
    Add-CargoBinToPath
  }

  $installed = (& rustup toolchain list) -join "`n"
  if ($installed -notmatch [regex]::Escape($toolchain)) {
    if ($Check) { Fail "toolchain $toolchain absente (installées : $installed)" }
    & rustup toolchain install $toolchain --profile minimal
  }
  $components = (& rustup component list --toolchain $toolchain --installed) -join "`n"
  foreach ($c in $versions.components) {
    if ($components -notmatch "(?m)^$([regex]::Escape($c))(-|$)") {
      if ($Check) { Fail "composant $c absent de la toolchain $toolchain" }
      & rustup component add $c --toolchain $toolchain
    }
  }
  foreach ($t in $versions.targets) {
    $targets = (& rustup target list --toolchain $toolchain --installed) -join "`n"
    if ($targets -notmatch "(?m)^$([regex]::Escape($t))$") {
      if ($Check) { Fail "cible $t absente de la toolchain $toolchain" }
      & rustup target add $t --toolchain $toolchain
    }
  }
  $rustc = (& rustup run $toolchain rustc --version)
  if ($rustc -notmatch [regex]::Escape($versions.rust)) {
    Fail "rustc inattendu : $rustc (attendu $($versions.rust))"
  }
  return $rustc
}

# Visual Studio Build Tools : version majeure épinglée et présence de link.exe (l'éditeur
# de liens MSVC est obligatoire pour un .sys, rust-lld ne convient pas).
function Require-BuildTools {
  $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
  if (-not (Test-Path $vswhere)) {
    Require-Admin-Install "Visual Studio Build Tools $($versions.visual_studio_build_tools) absents (vswhere introuvable)" $wingetBuildTools
  }
  $vs = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationVersion
  if (-not $vs) {
    Require-Admin-Install "Build Tools sans les outils C++ (VC.Tools.x86.x64)" $wingetBuildTools
  }
  if ($vs.Split(".")[0] -ne $versions.visual_studio_build_tools) {
    Fail "Build Tools $vs, attendu $($versions.visual_studio_build_tools).x"
  }
  $link = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find "VC\Tools\MSVC\*\bin\Hostx64\x64\link.exe"
  if (-not $link) {
    Require-Admin-Install "link.exe (MSVC x64) introuvable dans les Build Tools $vs" $wingetBuildTools
  }
  $sdkInclude = Join-Path (Get-KitsRoot) "Include\$($versions.windows_sdk)\um\windows.h"
  if (-not (Test-Path $sdkInclude)) {
    Require-Admin-Install "SDK Windows $($versions.windows_sdk) absent ($sdkInclude)" $wingetBuildTools
  }
  return $vs
}

function Get-KitsRoot {
  $props = Get-ItemProperty -Path $kitsRootKey -ErrorAction SilentlyContinue
  if ($null -eq $props -or -not ($props.PSObject.Properties.Name -contains "KitsRoot10")) { return $null }
  return $props.KitsRoot10
}

# WDK : racine des kits dans le registre (là où wdk-build la cherche) et fichiers utilisés
# par le workspace noyau (en-têtes et bibliothèques km, outils d'empaquetage, devgen).
function Require-Wdk {
  $root = Get-KitsRoot
  if (-not $root) {
    Require-Admin-Install "WDK $($versions.wdk) absent (KitsRoot10 introuvable dans $kitsRootKey)" $wingetWdk
  }
  $v = $versions.wdk
  $required = @(
    "Include\$v\km\portcls.h",
    "Lib\$v\km\x64\portcls.lib",
    "bin\$v\x64\stampinf.exe",
    "bin\$v\x86\inf2cat.exe",
    "Tools\$v\x64\devgen.exe"
  )
  foreach ($rel in $required) {
    $path = Join-Path $root $rel
    if (-not (Test-Path $path)) {
      Require-Admin-Install "WDK $v incomplet : $path manquant" $wingetWdk
    }
  }
  return $root
}

# LLVM : libclang.dll pour bindgen (LIBCLANG_PATH, sinon l'installation par défaut) et
# version épinglée (clang.exe du même dossier).
function Require-Llvm {
  $candidates = @()
  if ($env:LIBCLANG_PATH) { $candidates += $env:LIBCLANG_PATH }
  $candidates += "C:\Program Files\LLVM\bin"
  $bin = $null
  foreach ($dir in $candidates) {
    if (Test-Path (Join-Path $dir "libclang.dll")) { $bin = $dir; break }
  }
  if (-not $bin) {
    Require-Admin-Install "libclang.dll introuvable (cherché dans : $($candidates -join ', ')) ; LLVM $($versions.llvm) attendu, LLVM 22 casse les bindings de wdk-sys 0.5.1" $wingetLlvm
  }
  $clang = Join-Path $bin "clang.exe"
  if (-not (Test-Path $clang)) {
    Require-Admin-Install "clang.exe absent de $bin (installation LLVM incomplète)" $wingetLlvm
  }
  $line = (& $clang --version | Select-Object -First 1)
  if ($line -notmatch "clang version (\S+)") { Fail "version de clang illisible : $line" }
  $found = $Matches[1]
  if (-not $found.StartsWith($versions.llvm)) {
    Require-Admin-Install "LLVM $found dans $bin, attendu $($versions.llvm) : LLVM 22 casse les bindings de wdk-sys 0.5.1 (docs/windows-drivers-rs.md)" $wingetLlvm
  }
  return "$found ($bin)"
}

# cargo-wdk : sous-commande de build et d'empaquetage de windows-drivers-rs.
function Require-CargoWdk {
  Add-CargoBinToPath
  $expected = $versions.cargo_wdk
  $current = $null
  if (Get-Command cargo-wdk -ErrorAction SilentlyContinue) {
    $current = (& cargo wdk --version | Out-String).Trim()
  }
  if (-not $current -or $current -notmatch [regex]::Escape($expected)) {
    if ($Check) {
      $seen = if ($current) { $current } else { "absent" }
      Fail "cargo-wdk $expected attendu ($seen) : cargo install cargo-wdk --version $expected --locked"
    }
    & cargo install cargo-wdk --version $expected --locked
    if ($LASTEXITCODE -ne 0) { Fail "échec de l'installation de cargo-wdk $expected" }
    $current = (& cargo wdk --version | Out-String).Trim()
    if ($current -notmatch [regex]::Escape($expected)) { Fail "cargo-wdk inattendu après installation : $current" }
  }
  return $current
}

$rustcVersion = Require-Rust
$buildTools = Require-BuildTools
$wdkRoot = Require-Wdk
$llvm = Require-Llvm
$cargoWdk = Require-CargoWdk

$crates = ($versions.wdk_crates.PSObject.Properties | ForEach-Object { "$($_.Name) $($_.Value)" }) -join ", "
Write-Host "environnement Windows conforme :"
Write-Host "  rust        $rustcVersion (toolchain $toolchain)"
Write-Host "  Build Tools $buildTools (link.exe MSVC x64, SDK $($versions.windows_sdk))"
Write-Host "  WDK         $($versions.wdk) ($wdkRoot)"
Write-Host "  LLVM        $llvm"
Write-Host "  cargo-wdk   $cargoWdk"
Write-Host "  crates      $crates (épinglés dans drivers/windows/Cargo.toml)"
