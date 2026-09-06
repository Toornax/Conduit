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

Describe "Get-NamedPipeName" {
  It "extrait le nom court d'un canal local" {
    Get-NamedPipeName -Pipe '\\.\pipe\conduitdbg' | Should Be "conduitdbg"
  }
  It "accepte un nom de machine à la place du point" {
    Get-NamedPipeName -Pipe '\\HOTE\pipe\conduitdbg' | Should Be "conduitdbg"
  }
  It "accepte un nom nu" {
    Get-NamedPipeName -Pipe "conduitdbg" | Should Be "conduitdbg"
  }
  It "ignore les espaces autour" {
    Get-NamedPipeName -Pipe '  \\.\pipe\conduitdbg  ' | Should Be "conduitdbg"
  }
  It "garde un nom imbriqué tel qu'il apparaît dans l'espace de noms des canaux" {
    # vm-debug.ps1 compare les entrées de \\.\pipe\ avec cette fonction, et non avec
    # Split-Path -Leaf, qui rend une chaîne VIDE sur « \\.\pipe\nom » (vérifié).
    Get-NamedPipeName -Pipe '\\.\pipe\Winsock2\CatalogChangeListener-580-0' |
      Should Be 'Winsock2\CatalogChangeListener-580-0'
  }
  It "refuse ce qui n'est pas un canal nommé" {
    Get-NamedPipeName -Pipe "C:\temp\fichier" | Should BeNullOrEmpty
    Get-NamedPipeName -Pipe "" | Should BeNullOrEmpty
    Get-NamedPipeName -Pipe $null | Should BeNullOrEmpty
  }
}

Describe "Get-KdCommandLine" {
  $kdPath = "C:\Program Files (x86)\Windows Kits\10\Debuggers\x64\kd.exe"
  $pipe = '\\.\pipe\conduitdbg'
  $log = "C:\dev\target\debug-logs\kd.log"
  $kd = Get-KdCommandLine -KdPath $kdPath -Pipe $pipe -LogPath $log `
    -InitialCommands @("ed nt!Kd_IHVDRIVER_Mask 0xf", "g")

  It "ouvre le canal nommé en série, avec resets=0 et reconnect" {
    $kd.Arguments[0] | Should Be "-k"
    $kd.Arguments[1] | Should Be 'com:pipe,port=\\.\pipe\conduitdbg,resets=0,reconnect'
  }
  It "journalise avec -logo" {
    $index = [array]::IndexOf($kd.Arguments, "-logo")
    ($index -ge 0) | Should Be $true
    $kd.Arguments[$index + 1] | Should Be $log
  }
  It "joue les commandes initiales avec -c, séparées par un point-virgule" {
    $index = [array]::IndexOf($kd.Arguments, "-c")
    ($index -ge 0) | Should Be $true
    $kd.Arguments[$index + 1] | Should Be "ed nt!Kd_IHVDRIVER_Mask 0xf; g"
  }
  It "cite les arguments qui contiennent des espaces, et eux seuls" {
    $kd.ArgumentString.Contains('"ed nt!Kd_IHVDRIVER_Mask 0xf; g"') | Should Be $true
    $kd.ArgumentString.Contains("-k com:pipe") | Should Be $true
  }
  It "cite le chemin du débogueur dans la ligne complète" {
    $kd.CommandLine.StartsWith('"' + $kdPath + '"') | Should Be $true
  }
  It "n'ajoute pas -c sans commande initiale" {
    $sans = Get-KdCommandLine -KdPath $kdPath -Pipe $pipe -LogPath $log -InitialCommands @()
    ($sans.Arguments -contains "-c") | Should Be $false
    $vides = Get-KdCommandLine -KdPath $kdPath -Pipe $pipe -LogPath $log -InitialCommands @("", "   ")
    ($vides.Arguments -contains "-c") | Should Be $false
  }
  It "reprend le canal tel quel, quelle qu'en soit l'écriture" {
    $court = Get-KdCommandLine -KdPath $kdPath -Pipe "conduitdbg" -LogPath $log
    $court.Arguments[1] | Should Be "com:pipe,port=conduitdbg,resets=0,reconnect"
  }
}

Describe "Get-AccountName / Test-SameAccount" {
  It "retire le domaine ou la machine" {
    Get-AccountName -UserName "CONDUITTEST\nathan" | Should Be "nathan"
    Get-AccountName -UserName ".\nathan" | Should Be "nathan"
    Get-AccountName -UserName "nathan" | Should Be "nathan"
    Get-AccountName -UserName "nathan@exemple.local" | Should Be "nathan"
  }
  It "rend une chaîne vide sur une entrée vide" {
    Get-AccountName -UserName "" | Should Be ""
    Get-AccountName -UserName $null | Should Be ""
  }
  It "reconnaît le même compte écrit de trois façons" {
    (Test-SameAccount -Left "CONDUITTEST\nathan" -Right "nathan") | Should Be $true
    (Test-SameAccount -Left ".\nathan" -Right "CONDUITTEST\NATHAN") | Should Be $true
  }
  It "distingue deux comptes différents" {
    (Test-SameAccount -Left "CONDUITTEST\nathan" -Right "test") | Should Be $false
  }
  It "refuse de conclure sur une entrée vide" {
    (Test-SameAccount -Left "" -Right "nathan") | Should Be $false
    (Test-SameAccount -Left $null -Right $null) | Should Be $false
  }
}

Describe "Get-TaskRunAsUser" {
  It "qualifie par la machine invitée un compte non qualifié" {
    Get-TaskRunAsUser -UserName "nathan" -ComputerName "CONDUITTEST" | Should Be "CONDUITTEST\nathan"
  }
  It "remplace le point de « .\compte » par la machine" {
    Get-TaskRunAsUser -UserName ".\nathan" -ComputerName "CONDUITTEST" | Should Be "CONDUITTEST\nathan"
  }
  It "laisse intact un compte déjà qualifié" {
    Get-TaskRunAsUser -UserName "CONDUITTEST\nathan" -ComputerName "AUTRE" | Should Be "CONDUITTEST\nathan"
    Get-TaskRunAsUser -UserName "nathan@exemple.local" -ComputerName "AUTRE" | Should Be "nathan@exemple.local"
  }
  It "se contente du compte quand la machine est inconnue" {
    Get-TaskRunAsUser -UserName "nathan" -ComputerName "" | Should Be "nathan"
  }
  It "n'invente jamais de compte : il vient du -Credential" {
    Get-TaskRunAsUser -UserName "test" -ComputerName "CONDUITTEST" | Should Be "CONDUITTEST\test"
  }
}

Describe "Get-ConsoleSessionDecision" {
  $console = "CONDUITTEST\nathan"
  It "accepte une session console dont l'explorer n'est pas dans la session 0" {
    $d = Get-ConsoleSessionDecision -ConsoleUser $console -ExplorerSessions @(
      [pscustomobject]@{ UserName = $console; SessionId = 1 })
    $d.Ready | Should Be $true
    $d.SessionId | Should Be 1
    $d.Reason | Should Be ""
  }
  It "reconnaît l'utilisateur quelle que soit la qualification du nom" {
    $d = Get-ConsoleSessionDecision -ConsoleUser $console -ExplorerSessions @(
      [pscustomobject]@{ UserName = "nathan"; SessionId = 2 })
    $d.Ready | Should Be $true
    $d.SessionId | Should Be 2
  }
  It "refuse un explorer qui n'est QUE dans la session 0 (celle des services, sans audio)" {
    $d = Get-ConsoleSessionDecision -ConsoleUser $console -ExplorerSessions @(
      [pscustomobject]@{ UserName = $console; SessionId = 0 })
    $d.Ready | Should Be $false
    $d.SessionId | Should Be 0
    $d.Reason | Should Match "session 0"
  }
  It "refuse quand personne n'est ouvert à la console" {
    $d = Get-ConsoleSessionDecision -ConsoleUser "" -ExplorerSessions @(
      [pscustomobject]@{ UserName = $console; SessionId = 1 })
    $d.Ready | Should Be $false
    $d.Reason | Should Match "console"
  }
  It "refuse quand l'utilisateur ouvert n'a pas de bureau chargé" {
    $d = Get-ConsoleSessionDecision -ConsoleUser $console -ExplorerSessions @()
    $d.Ready | Should Be $false
    ($d.SessionId -eq -1) | Should Be $true
    $d.Reason | Should Match "explorer"
  }
  It "ignore l'explorer d'un AUTRE utilisateur" {
    $d = Get-ConsoleSessionDecision -ConsoleUser $console -ExplorerSessions @(
      [pscustomobject]@{ UserName = "CONDUITTEST\test"; SessionId = 3 })
    $d.Ready | Should Be $false
  }
  It "supporte une liste nulle et des entrées incomplètes" {
    (Get-ConsoleSessionDecision -ConsoleUser $console -ExplorerSessions $null).Ready | Should Be $false
    $d = Get-ConsoleSessionDecision -ConsoleUser $console -ExplorerSessions @(
      $null,
      [pscustomobject]@{ UserName = $console },
      [pscustomobject]@{ UserName = $console; SessionId = 1 })
    $d.Ready | Should Be $true
  }
}

Describe "Get-ConsoleRunScript" {
  $texte = Get-ConsoleRunScript -Executable "C:\ConduitTest\conduit-looptest.exe" `
    -Arguments @("--repeat", "10") -OutputPath "C:\ConduitTest\sortie.txt" `
    -ExitCodePath "C:\ConduitTest\code.txt" -WorkingDirectory "C:\ConduitTest"
  $lignes = $texte -split "`r`n"

  It "se termine par des fins de ligne Windows" {
    $texte.EndsWith("`r`n") | Should Be $true
    $texte.Contains("`n`n") | Should Be $false
  }
  It "se place dans le dossier de travail" {
    ($lignes -contains 'cd /d "C:\ConduitTest"') | Should Be $true
  }
  It "redirige sortie et erreur vers le fichier de sortie" {
    $commande = @($lignes | Where-Object { $_ -like "*conduit-looptest.exe*" })[0]
    $commande.Contains('"C:\ConduitTest\conduit-looptest.exe" --repeat 10') | Should Be $true
    $commande.Contains('>"C:\ConduitTest\sortie.txt" 2>&1') | Should Be $true
  }
  It "écrit le code de retour AVEC la redirection en tête (sinon cmd lit le chiffre comme un flux)" {
    $sentinelle = @($lignes | Where-Object { $_ -like "*code.txt*" })[0]
    $sentinelle.StartsWith('>"C:\ConduitTest\code.txt" echo ') | Should Be $true
    ($lignes -contains "set CONDUIT_CODE=%ERRORLEVEL%") | Should Be $true
  }
  It "capture le code AVANT toute autre commande" {
    $iCommande = [array]::IndexOf($lignes, @($lignes | Where-Object { $_ -like "*conduit-looptest.exe*" })[0])
    $iCode = [array]::IndexOf($lignes, "set CONDUIT_CODE=%ERRORLEVEL%")
    ($iCode -eq $iCommande + 1) | Should Be $true
  }
  It "cite les arguments à espaces et double les pour-cent" {
    $t = Get-ConsoleRunScript -Executable "C:\Program Files\outil.exe" `
      -Arguments @("--nom", "Conduit 1", "--gabarit", "100%") `
      -OutputPath "C:\o.txt" -ExitCodePath "C:\c.txt"
    $t.Contains('"C:\Program Files\outil.exe" --nom "Conduit 1" --gabarit 100%%') | Should Be $true
  }
  It "accepte une commande sans argument ni dossier de travail" {
    $t = Get-ConsoleRunScript -Executable "C:\a.exe" -Arguments @() -OutputPath "C:\o.txt" -ExitCodePath "C:\c.txt"
    $t.Contains("cd /d") | Should Be $false
    $t.Contains('"C:\a.exe" >"C:\o.txt" 2>&1') | Should Be $true
  }
}

Describe "Test-ConduitGhostDevice" {
  $audio = "{c166523c-fe0c-4a94-a586-f1a80cfbbf3e}"
  $media = "{4d36e96c-e325-11ce-bfc1-08002be10318}"

  It "retire un périphérique fantôme Root\ConduitCable" {
    (Test-ConduitGhostDevice -InstanceId "ROOT\CONDUITCABLE\0000" -FriendlyName "Conduit — câbles audio virtuels" `
      -ClassGuid $media -Present $false) | Should Be $true
  }
  It "retire un endpoint audio fantôme qui porte notre nom" {
    (Test-ConduitGhostDevice -InstanceId "SWD\MMDEVAPI\{0.0.0.00000000}.{9d2a6c1e-5a0b}" `
      -FriendlyName "Conduit 1 (Conduit — câbles audio virtuels)" -ClassGuid $audio -Present $false) | Should Be $true
  }
  It "reconnaît le GUID de classe sans accolades" {
    (Test-ConduitGhostDevice -InstanceId "SWD\AUTRE\1" -FriendlyName "Conduit 1" `
      -ClassGuid "c166523c-fe0c-4a94-a586-f1a80cfbbf3e" -Present $false) | Should Be $true
  }
  It "ne touche JAMAIS à un périphérique présent" {
    (Test-ConduitGhostDevice -InstanceId "ROOT\CONDUITCABLE\0000" -FriendlyName "Conduit 1" `
      -ClassGuid $media -Present $true) | Should Be $false
    (Test-ConduitGhostDevice -InstanceId "SWD\MMDEVAPI\{0.0.0.1}" -FriendlyName "Conduit 1" `
      -ClassGuid $audio -Present $true) | Should Be $false
  }
  It "ne touche pas aux endpoints fantômes d'une vraie carte son" {
    (Test-ConduitGhostDevice -InstanceId "SWD\MMDEVAPI\{0.0.0.00000000}.{aaaa}" `
      -FriendlyName "Haut-parleurs (Realtek High Definition Audio)" -ClassGuid $audio -Present $false) | Should Be $false
  }
  It "ne touche pas à un périphérique d'un autre fournisseur, même fantôme" {
    (Test-ConduitGhostDevice -InstanceId "ROOT\MEDIA\0001" -FriendlyName "Périphérique audio virtuel X" `
      -ClassGuid $media -Present $false) | Should Be $false
  }
  It "n'accepte un nom manquant que pour nos propres Root\ConduitCable" {
    (Test-ConduitGhostDevice -InstanceId "ROOT\CONDUITCABLE\0000" -FriendlyName $null -ClassGuid $null -Present $false) | Should Be $true
    (Test-ConduitGhostDevice -InstanceId "SWD\MMDEVAPI\{0.0.0.1}" -FriendlyName $null -ClassGuid $audio -Present $false) | Should Be $false
  }
  It "ignore la casse de l'identifiant d'instance" {
    (Test-ConduitGhostDevice -InstanceId "Root\ConduitCable\0000" -FriendlyName "" -ClassGuid "" -Present $false) | Should Be $true
  }
  It "refuse un identifiant vide" {
    (Test-ConduitGhostDevice -InstanceId "" -FriendlyName "Conduit 1" -ClassGuid $audio -Present $false) | Should Be $false
  }
  It "reste utilisable une fois transportée dans l'invité (aucune variable de module)" {
    # vm-cycle.ps1 l'envoie par Get-FunctionSource : elle doit se suffire à elle-même.
    $sb = [scriptblock]::Create((Get-FunctionSource -Name Test-ConduitGhostDevice) +
      "`nTest-ConduitGhostDevice -InstanceId 'ROOT\CONDUITCABLE\0000' -FriendlyName '' -ClassGuid '' -Present `$false")
    (& $sb) | Should Be $true
  }
}

Describe "Surface exportée : débogage série et session console" {
  $module = Get-Module vm-common
  It "exporte les fonctions pures des nouveaux scripts" {
    foreach ($name in @("Get-NamedPipeName", "Get-KdCommandLine", "Get-AccountName",
                        "Test-SameAccount", "Get-TaskRunAsUser", "Get-ConsoleSessionDecision",
                        "Get-ConsoleRunScript", "Test-ConduitGhostDevice", "Test-DevnodeGone",
                        "Get-DebuggerAttachWarning")) {
      $module.ExportedFunctions.ContainsKey($name) | Should Be $true
    }
  }
  It "exporte les enveloppes système qu'elles accompagnent" {
    foreach ($name in @("Get-KdPath", "Wait-VMOff")) {
      $module.ExportedFunctions.ContainsKey($name) | Should Be $true
    }
  }
  It "n'exporte pas l'aide interne de citation" {
    $module.ExportedFunctions.ContainsKey("Format-NativeArgument") | Should Be $false
  }
}

Describe "Test-DevnodeGone" {
  $id = "SWD\DEVGEN\{0c9acf52-1234-4a0b-9c3d-000000000001}"
  $autre = "SWD\DEVGEN\{0c9acf52-1234-4a0b-9c3d-000000000002}"

  It "voit disparu un identifiant absent de l'inventaire" {
    (Test-DevnodeGone -InstanceId $id -Devices @()) | Should Be $true
    (Test-DevnodeGone -InstanceId $id -Devices $null) | Should Be $true
  }
  It "voit encore là un périphérique présent qui porte cet identifiant" {
    (Test-DevnodeGone -InstanceId $id -Devices @(
      [pscustomobject]@{ InstanceId = $id; Present = $true })) | Should Be $false
  }
  It "accepte un fantôme (Present à faux) comme disparu" {
    # Un devnode non présent n'est plus un devnode que PnP démarre : la course est finie.
    (Test-DevnodeGone -InstanceId $id -Devices @(
      [pscustomobject]@{ InstanceId = $id; Present = $false })) | Should Be $true
  }
  It "ne décide JAMAIS sur le libellé Status, qui est traduit" {
    # Present (booléen) prime ; Status ne doit avoir aucune influence, dans un sens comme
    # dans l'autre — c'est tout l'objet de cette fonction.
    (Test-DevnodeGone -InstanceId $id -Devices @(
      [pscustomobject]@{ InstanceId = $id; Present = $true; Status = "Erreur" })) | Should Be $false
    (Test-DevnodeGone -InstanceId $id -Devices @(
      [pscustomobject]@{ InstanceId = $id; Present = $false; Status = "OK" })) | Should Be $true
  }
  It "ignore les périphériques qui portent un autre identifiant" {
    (Test-DevnodeGone -InstanceId $id -Devices @(
      [pscustomobject]@{ InstanceId = $autre; Present = $true },
      [pscustomobject]@{ InstanceId = "ROOT\MEDIA\0001"; Present = $true })) | Should Be $true
  }
  It "ignore la casse et les espaces de l'identifiant" {
    (Test-DevnodeGone -InstanceId $id -Devices @(
      [pscustomobject]@{ InstanceId = $id.ToLower(); Present = $true })) | Should Be $false
    (Test-DevnodeGone -InstanceId " $id " -Devices @(
      [pscustomobject]@{ InstanceId = "$id`t"; Present = $true })) | Should Be $false
  }
  It "tient pour PRÉSENT un objet sans propriété Present (prudence : on n'invente rien)" {
    (Test-DevnodeGone -InstanceId $id -Devices @(
      [pscustomobject]@{ InstanceId = $id })) | Should Be $false
    (Test-DevnodeGone -InstanceId $id -Devices @(
      [pscustomobject]@{ InstanceId = $id; Present = $null })) | Should Be $false
  }
  It "supporte les entrées nulles ou sans identifiant" {
    (Test-DevnodeGone -InstanceId $id -Devices @($null, [pscustomobject]@{ Present = $true })) | Should Be $true
    (Test-DevnodeGone -InstanceId $id -Devices @(
      $null,
      [pscustomobject]@{ InstanceId = $id; Present = $true })) | Should Be $false
  }
  It "n'attend rien d'un identifiant vide" {
    (Test-DevnodeGone -InstanceId "" -Devices @()) | Should Be $true
    (Test-DevnodeGone -InstanceId $null -Devices @()) | Should Be $true
  }
  It "reste utilisable une fois transportée dans l'invité (aucune variable de module)" {
    # vm-cycle.ps1 l'envoie par Get-FunctionSource : elle doit se suffire à elle-même.
    $sb = [scriptblock]::Create((Get-FunctionSource -Name Test-DevnodeGone) +
      "`nTest-DevnodeGone -InstanceId 'SWD\DEVGEN\{1}' -Devices @()")
    (& $sb) | Should Be $true
  }
}

Describe "Get-DebuggerAttachWarning" {
  $nom = "ConduitTest"

  It "avertit quand la VM tourne et que -StartVM n'est pas passé" {
    $message = Get-DebuggerAttachWarning -State "Running" -StartVM $false -Name $nom
    [string]::IsNullOrEmpty($message) | Should Be $false
    $message.Contains($nom) | Should Be $true
  }
  It "dit ce qui a été mesuré et le remède" {
    $message = Get-DebuggerAttachWarning -State "Running" -StartVM $false -Name $nom
    $message.Contains("Waiting to reconnect") | Should Be $true
    $message.Contains("-StartVM") | Should Be $true
  }
  It "ne dit rien quand -StartVM impose déjà l'ordre" {
    (Get-DebuggerAttachWarning -State "Running" -StartVM $true -Name $nom) | Should Be ""
  }
  It "ne dit rien sur une VM qui ne tourne pas" {
    foreach ($etat in @("Off", "Paused", "Saved", "Starting")) {
      (Get-DebuggerAttachWarning -State $etat -StartVM $false -Name $nom) | Should Be ""
    }
  }
  It "ne dit rien quand l'état n'a pas pu être lu" {
    (Get-DebuggerAttachWarning -State "" -StartVM $false -Name $nom) | Should Be ""
    (Get-DebuggerAttachWarning -State $null -StartVM $false -Name $nom) | Should Be ""
  }
  It "compare le NOM du membre d'énumération, casse et espaces indifférents" {
    # VMState est une énumération .NET : son nom n'est pas traduit, contrairement aux
    # libellés du gestionnaire Hyper-V. C'est ce nom que l'on compare, jamais un affichage.
    [string]::IsNullOrEmpty((Get-DebuggerAttachWarning -State " running " -StartVM $false -Name $nom)) | Should Be $false
    [string]::IsNullOrEmpty((Get-DebuggerAttachWarning -State "RUNNING" -StartVM $false -Name $nom)) | Should Be $false
  }
  It "accepte un nom de VM vide" {
    [string]::IsNullOrEmpty((Get-DebuggerAttachWarning -State "Running" -StartVM $false -Name "")) | Should Be $false
  }
}
