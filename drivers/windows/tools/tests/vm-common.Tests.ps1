<#
.SYNOPSIS
  Tests Pester (syntaxe 3/4, celle du Pester livré avec Windows) des fonctions pures de
  vm-common.psm1. Lancer : Invoke-Pester drivers\windows\tools\tests
  Rien ici ne touche à Hyper-V, au registre ni à un exécutable natif.
#>
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

Import-Module (Join-Path (Split-Path -Parent $PSScriptRoot) "vm-common.psm1") -Force

# SID du groupe Builtin\Hyper-V Administrators. Répété ici en dur À DESSEIN : si le module
# le changeait par accident, ces tests doivent virer au rouge.
$hyperVSid = "S-1-5-32-578"
$accessDenied = -2147024891   # 0x80070005, E_ACCESSDENIED en Int32 signé.

Describe "Test-SidPresent" {
  It "trouve un SID présent dans la liste" {
    (Test-SidPresent -Sid $hyperVSid -TokenSids @("S-1-1-0", $hyperVSid, "S-1-5-32-545")) | Should Be $true
  }
  It "ne trouve pas un SID absent" {
    (Test-SidPresent -Sid $hyperVSid -TokenSids @("S-1-1-0", "S-1-5-32-545")) | Should Be $false
  }
  It "ignore la casse (un SID textuel n'est pas sensible à la casse)" {
    (Test-SidPresent -Sid $hyperVSid -TokenSids @("s-1-5-32-578")) | Should Be $true
    (Test-SidPresent -Sid "s-1-5-32-578" -TokenSids @($hyperVSid)) | Should Be $true
  }
  It "ignore les espaces autour des SID" {
    (Test-SidPresent -Sid " $hyperVSid " -TokenSids @("  $hyperVSid`t")) | Should Be $true
  }
  It "renvoie faux sur une liste vide" {
    (Test-SidPresent -Sid $hyperVSid -TokenSids @()) | Should Be $false
  }
  It "renvoie faux sur une liste nulle ou contenant des entrées vides" {
    (Test-SidPresent -Sid $hyperVSid -TokenSids $null) | Should Be $false
    (Test-SidPresent -Sid $hyperVSid -TokenSids @($null, "", "S-1-1-0")) | Should Be $false
  }
  It "trouve quand même le SID au milieu d'entrées vides" {
    (Test-SidPresent -Sid $hyperVSid -TokenSids @($null, "", $hyperVSid)) | Should Be $true
  }
  It "ne confond pas un SID avec un préfixe d'un autre" {
    (Test-SidPresent -Sid "S-1-5-32-57" -TokenSids @($hyperVSid)) | Should Be $false
  }
}

Describe "Get-TokenSid" {
  # Enveloppe système : on ne vérifie que la forme, jamais une appartenance précise (elle
  # dépend de la session qui lance les tests).
  It "renvoie au moins un SID, tous sous forme textuelle S-…" {
    $sids = @(Get-TokenSid)
    $sids.Count -gt 0 | Should Be $true
    foreach ($sid in $sids) { $sid | Should Match '^S-\d+-\d+' }
  }
  It "contient le SID du compte courant" {
    $me = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    (Test-SidPresent -Sid $me -TokenSids (Get-TokenSid)) | Should Be $true
  }
  It "n'expose aucun nom de groupe (les noms sont traduits, jamais utilisés pour décider)" {
    foreach ($sid in @(Get-TokenSid)) { $sid | Should Not Match '\\' }
  }
}

Describe "Test-HyperVOperator" {
  It "renvoie un booléen" {
    (Test-HyperVOperator) -is [bool] | Should Be $true
  }
  It "est vrai dès que la session est élevée" {
    if (Test-Elevated) { (Test-HyperVOperator) | Should Be $true }
    else { (Test-HyperVOperator) | Should Be (Test-SidPresent -Sid $hyperVSid -TokenSids (Get-TokenSid)) }
  }
}

Describe "Get-HyperVAccessHint" {
  $hint = Get-HyperVAccessHint
  It "cite le SID du groupe, pas son nom traduit" {
    ($hint -like "*$hyperVSid*") | Should Be $true
  }
  It "dit qu'il faut rouvrir la session Windows" {
    ($hint -like "*rouvrir*") | Should Be $true
    ($hint -like "*session*") | Should Be $true
  }
  It "prévient que redémarrer le terminal ne suffit pas" {
    ($hint -like "*terminal ne suffit pas*") | Should Be $true
  }
}

Describe "Get-HyperVAccessMessage" {
  $msg = Get-HyperVAccessMessage -Reason "création d'une VM Hyper-V" -UserName "nathan"
  It "cite le SID du groupe" {
    ($msg -like "*$hyperVSid*") | Should Be $true
  }
  It "reprend la raison passée" {
    ($msg -like "*création d'une VM Hyper-V*") | Should Be $true
  }
  It "reprend le nom d'utilisateur passé" {
    ($msg -like "*nathan*") | Should Be $true
  }
  It "propose le remède « relancer en administrateur »" {
    ($msg -like "*administrateur*") | Should Be $true
  }
  It "propose le remède « rejoindre le groupe », par le SID" {
    ($msg -like "*Add-LocalGroupMember -SID $hyperVSid -Member*") | Should Be $true
  }
  It "exige la fermeture puis la réouverture de la session Windows" {
    ($msg -like "*FERMER ET ROUVRIR LA SESSION WINDOWS*") | Should Be $true
    ($msg -like "*redémarrer le terminal ne*suffit pas*") | Should Be $true
  }
  It "donne bien les DEUX remèdes numérotés" {
    ($msg -like "*1.*") | Should Be $true
    ($msg -like "*2.*") | Should Be $true
  }
  It "remplace un nom d'utilisateur vide par un marqueur" {
    $vide = Get-HyperVAccessMessage -Reason "test" -UserName ""
    ($vide -like "*<votre compte>*") | Should Be $true
  }
}

Describe "Test-AccessDeniedError" {
  It "reconnaît un HRESULT 0x80070005 direct" {
    $e = New-Object System.Runtime.InteropServices.COMException("refus", $accessDenied)
    (Test-AccessDeniedError -ErrorRecord $e) | Should Be $true
  }
  It "reconnaît un HRESULT 0x80070005 imbriqué" {
    $inner = New-Object System.Runtime.InteropServices.COMException("refus", $accessDenied)
    $outer = New-Object System.InvalidOperationException("opération Hyper-V impossible", $inner)
    (Test-AccessDeniedError -ErrorRecord $outer) | Should Be $true
  }
  It "reconnaît un HRESULT 0x80070005 imbriqué à deux niveaux" {
    $inner = New-Object System.Runtime.InteropServices.COMException("refus", $accessDenied)
    $mid = New-Object System.InvalidOperationException("intermédiaire", $inner)
    $outer = New-Object System.InvalidOperationException("dehors", $mid)
    (Test-AccessDeniedError -ErrorRecord $outer) | Should Be $true
  }
  It "reconnaît le type UnauthorizedAccessException" {
    (Test-AccessDeniedError -ErrorRecord (New-Object System.UnauthorizedAccessException("nope"))) | Should Be $true
  }
  It "reconnaît le type SecurityException" {
    (Test-AccessDeniedError -ErrorRecord (New-Object System.Security.SecurityException("nope"))) | Should Be $true
  }
  It "accepte un ErrorRecord aussi bien qu'une exception" {
    $e = New-Object System.Runtime.InteropServices.COMException("refus", $accessDenied)
    $rec = New-Object System.Management.Automation.ErrorRecord($e, "id", [System.Management.Automation.ErrorCategory]::NotSpecified, $null)
    (Test-AccessDeniedError -ErrorRecord $rec) | Should Be $true
  }
  It "reconnaît la catégorie PermissionDenied (énumération, pas du texte)" {
    $e = New-Object System.Exception("banal")
    $rec = New-Object System.Management.Automation.ErrorRecord($e, "id", [System.Management.Automation.ErrorCategory]::PermissionDenied, $null)
    (Test-AccessDeniedError -ErrorRecord $rec) | Should Be $true
  }
  It "ne se déclenche pas sur une autre erreur" {
    (Test-AccessDeniedError -ErrorRecord (New-Object System.Exception("banal"))) | Should Be $false
    $rec = New-Object System.Management.Automation.ErrorRecord(
      (New-Object System.IO.FileNotFoundException("absent")), "id",
      [System.Management.Automation.ErrorCategory]::ObjectNotFound, $null)
    (Test-AccessDeniedError -ErrorRecord $rec) | Should Be $false
  }
  It "ne se fie pas au texte du message (il est traduit)" {
    # Un message qui parle d'accès refusé sans HRESULT ni type de refus : faux.
    (Test-AccessDeniedError -ErrorRecord (New-Object System.Exception("Accès refusé. Access is denied."))) | Should Be $false
  }
  It "accepte `$null" {
    (Test-AccessDeniedError -ErrorRecord $null) | Should Be $false
  }
}

Describe "Invoke-HyperVChecked" {
  It "renvoie la sortie du bloc quand tout va bien" {
    (Invoke-HyperVChecked -What "test" -Script { 42 }) | Should Be 42
  }
  It "renvoie une collection intacte" {
    @(Invoke-HyperVChecked -What "test" -Script { 1, 2, 3 }).Count | Should Be 3
  }
  It "ajoute l'indice de réouverture de session sur un refus d'accès" {
    $script = { throw (New-Object System.Runtime.InteropServices.COMException("refus", -2147024891)) }
    { Invoke-HyperVChecked -What "inventaire des VM" -Script $script } | Should Throw "inventaire des VM"
    $caught = $null
    try { Invoke-HyperVChecked -What "inventaire des VM" -Script $script } catch { $caught = "$_" }
    ($caught -like "*$hyperVSid*") | Should Be $true
    ($caught -like "*rouvrir*") | Should Be $true
  }
  It "relance telle quelle une erreur qui n'est pas un refus d'accès" {
    { Invoke-HyperVChecked -What "test" -Script { throw "panne quelconque" } } | Should Throw "panne quelconque"
    $caught = $null
    try { Invoke-HyperVChecked -What "test" -Script { throw "panne quelconque" } } catch { $caught = "$_" }
    ($caught -like "*rouvrir*") | Should Be $false
  }
}

Describe "Surface exportée de vm-common" {
  $module = Get-Module vm-common
  It "exporte les fonctions d'accès à Hyper-V" {
    foreach ($name in @("Test-SidPresent", "Get-TokenSid", "Test-HyperVOperator",
                        "Get-HyperVAccessHint", "Get-HyperVAccessMessage",
                        "Assert-HyperVAccess", "Test-AccessDeniedError", "Invoke-HyperVChecked")) {
      $module.ExportedFunctions.ContainsKey($name) | Should Be $true
    }
  }
  It "garde Test-Elevated (une opération pourrait encore exiger l'élévation)" {
    $module.ExportedFunctions.ContainsKey("Test-Elevated") | Should Be $true
  }
  It "n'exporte plus Assert-Elevated (remplacée par Assert-HyperVAccess)" {
    $module.ExportedFunctions.ContainsKey("Assert-Elevated") | Should Be $false
  }
}

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
