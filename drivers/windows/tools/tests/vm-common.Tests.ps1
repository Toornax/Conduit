<#
.SYNOPSIS
  Tests Pester (syntaxe 3/4, celle du Pester livré avec Windows) des fonctions pures de
  vm-common.psm1. Lancer : Invoke-Pester drivers\windows\tools\tests
  Rien ici ne touche à Hyper-V, au registre ni à un exécutable natif.
#>
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Import-Module (Join-Path (Split-Path -Parent $PSScriptRoot) "vm-common.psm1") -Force

Describe "New-DebugKey / Test-DebugKey" {
  It "produit quatre groupes de neuf caractères base 36" {
    $key = New-DebugKey
    ($key -match '^[0-9a-z]{9}\.[0-9a-z]{9}\.[0-9a-z]{9}\.[0-9a-z]{9}$') | Should Be $true
    (Test-DebugKey -Key $key) | Should Be $true
  }
  It "ne produit pas deux fois la même clé" {
    (New-DebugKey) -ne (New-DebugKey) | Should Be $true
  }
  It "refuse une clé mal formée" {
    (Test-DebugKey -Key "abc.def") | Should Be $false
    (Test-DebugKey -Key "") | Should Be $false
    (Test-DebugKey -Key "a.b.c.d e") | Should Be $false
  }
}

Describe "ConvertFrom-DevgenAddOutput" {
  It "extrait l'identifiant d'instance SWD" {
    $out = @("Device node created. Instance ID: SWD\DEVGEN\{9d2a6c1e-5a0b-4d1f-8f5e-0123456789ab}")
    ConvertFrom-DevgenAddOutput -Output $out | Should Be 'SWD\DEVGEN\{9d2a6c1e-5a0b-4d1f-8f5e-0123456789ab}'
  }
  It "extrait un identifiant ROOT quel que soit le libellé" {
    $out = @("Nœud créé.", "ID d'instance : ROOT\ConduitCable\0000", "")
    ConvertFrom-DevgenAddOutput -Output $out | Should Be 'ROOT\ConduitCable\0000'
  }
  It "renvoie null sans identifiant" {
    ConvertFrom-DevgenAddOutput -Output @("Failed to create device node.") | Should BeNullOrEmpty
    ConvertFrom-DevgenAddOutput -Output @() | Should BeNullOrEmpty
  }
}

Describe "Find-PublishedInf" {
  $enum = @(
    "Microsoft PnP Utility",
    "",
    "Published Name:     oem3.inf",
    "Original Name:      prnms009.inf",
    "Provider Name:      Microsoft",
    "Class Name:         Printers",
    "",
    "Published Name:     oem12.inf",
    "Original Name:      conduit_kmd.inf",
    "Provider Name:      Conduit",
    "Class Name:         Sound, video and game controllers",
    "Driver Version:     09/05/2026 0.1.0.0",
    "Signer Name:        WDRLocalTestCert"
  )
  It "retrouve l'oemN.inf du paquet Conduit" {
    Find-PublishedInf -Output $enum -OriginalName "conduit_kmd.inf" | Should Be "oem12.inf"
  }
  It "ignore les autres paquets" {
    @(Find-PublishedInf -Output $enum -OriginalName "absent.inf").Count | Should Be 0
  }
  It "ne dépend pas de la langue des libellés" {
    $fr = @("Nom publié :        oem7.inf", "Nom d'origine :     CONDUIT_KMD.INF")
    Find-PublishedInf -Output $fr -OriginalName "conduit_kmd.inf" | Should Be "oem7.inf"
  }
  It "renvoie tous les doublons" {
    $double = $enum + @("", "Published Name:     oem13.inf", "Original Name:      conduit_kmd.inf")
    @(Find-PublishedInf -Output $double -OriginalName "conduit_kmd.inf") | Should Be @("oem12.inf", "oem13.inf")
  }
}

Describe "Get-CycleSummary" {
  It "calcule nombre, moyenne, min et max" {
    $s = Get-CycleSummary -Durations @(1.0, 2.0, 3.0)
    $s.Count | Should Be 3
    $s.MeanSeconds | Should Be 2
    $s.MinSeconds | Should Be 1
    $s.MaxSeconds | Should Be 3
  }
  It "accepte une liste vide" {
    (Get-CycleSummary -Durations @()).Count | Should Be 0
  }
}

Describe "Invoke-NativeChecked" {
  It "renvoie la sortie d'une commande qui réussit" {
    $out = Invoke-NativeChecked -Exe "cmd.exe" -Arguments @("/c", "echo bonjour")
    ($out -join "") | Should Match "bonjour"
  }
  It "échoue avec la sortie d'une commande qui échoue" {
    { Invoke-NativeChecked -Exe "cmd.exe" -Arguments @("/c", "echo raison & exit 7") } | Should Throw "code 7"
  }
}

Describe "Get-FunctionSource" {
  It "produit une définition rechargeable" {
    $src = Get-FunctionSource -Name Find-PublishedInf
    $src | Should Match '^function Find-PublishedInf \{'
    $sb = [scriptblock]::Create($src)
    { . $sb } | Should Not Throw
  }
}
