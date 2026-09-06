<#
.SYNOPSIS
  Fonctions partagées par les scripts de VM (vm-new.ps1, vm-prepare.ps1, vm-cycle.ps1).
.DESCRIPTION
  Les fonctions pures (analyse de sorties, génération de clé, décision d'accès) sont
  testées par tests\vm-common.Tests.ps1 (Pester 3/4). Les enveloppes qui touchent au
  système (jeton courant, registre, réseau de l'hôte, Hyper-V) ne sont pas testées : la
  décision en est systématiquement extraite dans une fonction pure à laquelle on passe
  les données lues, pour qu'elle soit testable sans jeton ni hyperviseur réels.

    Fonction pure (testée)                | Enveloppe système
    --------------------------------------|--------------------------------
    Test-SidPresent                       | Get-TokenSid
    Get-HyperVAccessMessage               | Assert-HyperVAccess
    Get-HyperVAccessHint                  | Invoke-HyperVChecked
    Test-AccessDeniedError                | —
#>
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# Builtin\Hyper-V Administrators : « complete and unrestricted access to all features of
# Hyper-V ». Appartenir à ce groupe suffit pour piloter Hyper-V, l'élévation n'est pas
# requise :
#   - « Ensure that your user account belongs to the Administrators group or the Hyper-V
#     Administrators group »
#     https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/manage/remotely-manage-hyper-v-hosts
#   - PowerShell Direct (Invoke-Command -VMName, New-PSSession -VMName,
#     Copy-Item -To/FromSession) : « You must be logged into the host computer as a
#     Hyper-V administrator »
#     https://learn.microsoft.com/en-us/windows-server/virtualization/hyper-v/powershell-direct
#   - Table des SID connus (S-1-5-32-578 = Builtin\Hyper-V Administrators)
#     https://learn.microsoft.com/en-us/windows-server/identity/ad-ds/manage/understand-security-identifiers
#
# Toujours par le SID, JAMAIS par le nom du groupe : « Hyper-V Administrators » est
# traduit selon la langue de Windows (« Administrateurs Hyper-V » en français). C'est
# exactement le piège qui avait cassé la création de VM avant le commit 25b3557, où le
# composant d'intégration était cherché par son nom traduit.
$script:HyperVAdminsSid = "S-1-5-32-578"

function Test-Elevated {
  <#
  .SYNOPSIS
    Vrai si le processus courant appartient au groupe Administrateurs.
  .DESCRIPTION
    Conservée bien que les scripts de VM n'exigent plus l'élévation (voir
    Test-HyperVOperator) : elle resservira si une opération précise s'avère l'exiger.
  #>
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = New-Object Security.Principal.WindowsPrincipal($identity)
  return $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
}

function Test-SidPresent {
  <#
  .SYNOPSIS
    Vrai si $Sid figure dans la liste de SID fournie (comparaison insensible à la casse).
  .DESCRIPTION
    Fonction pure : la lecture du jeton est faite à part (Get-TokenSid), pour que la
    décision d'accès soit testable sans jeton réel. Les SID sont des chaînes ASCII dont
    la casse n'est pas significative (« s-1-5-32-578 » désigne le même groupe).
  .PARAMETER Sid
    SID recherché, sous forme textuelle (S-1-5-32-578).
  .PARAMETER TokenSids
    SID présents dans le jeton, sous forme textuelle. Liste vide, $null ou contenant des
    entrées vides acceptés (une lecture de jeton peut légitimement ne rien rendre).
  #>
  param(
    [Parameter(Mandatory)][string]$Sid,
    [Parameter(Mandatory)][AllowNull()][AllowEmptyCollection()][AllowEmptyString()][string[]]$TokenSids
  )
  $wanted = $Sid.Trim()
  foreach ($candidate in @($TokenSids)) {
    if ($null -eq $candidate) { continue }
    if ($candidate.Trim() -ieq $wanted) { return $true }
  }
  return $false
}

function Get-TokenSid {
  <#
  .SYNOPSIS
    SID du jeton EFFECTIF du processus courant (compte + groupes), sous forme textuelle.
  .DESCRIPTION
    WindowsIdentity.Groups n'expose que les SID réellement actifs : les SID marqués
    « refus uniquement » par le filtrage UAC en sont exclus (vérifié : en session non
    élevée, S-1-5-32-544 apparaît dans `whoami /groups` mais pas ici). C'est la propriété
    voulue : la documentation ne dit pas si S-1-5-32-578 survit à ce filtrage, mais le
    code n'en dépend pas puisqu'il lit le jeton effectif. Si le SID était filtré,
    Test-HyperVOperator renverrait faux et l'utilisateur verrait le message d'aide :
    dégradation propre, jamais de faux positif.
  #>
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $sids = New-Object System.Collections.Generic.List[string]
  if ($identity.User) { $sids.Add($identity.User.Value) }
  foreach ($group in @($identity.Groups)) {
    if ($null -eq $group) { continue }
    # Un SID sans traduction possible lève sur .Translate, jamais sur .Value : on ne
    # traduit pas, justement (les noms de groupes sont localisés).
    $sids.Add($group.Value)
  }
  return $sids.ToArray()
}

function Test-HyperVOperator {
  <#
  .SYNOPSIS
    Vrai si la session courante peut piloter Hyper-V : élevée (groupe Administrateurs),
    ou porteuse du SID du groupe Administrateurs Hyper-V (S-1-5-32-578).
  #>
  if (Test-Elevated) { return $true }
  return (Test-SidPresent -Sid $script:HyperVAdminsSid -TokenSids (Get-TokenSid))
}

function Get-HyperVAccessHint {
  <#
  .SYNOPSIS
    Indice à ajouter à un refus d'accès venu d'Hyper-V : la cause la plus probable est
    une appartenance au groupe acquise sans réouverture de session.
  .DESCRIPTION
    Fonction pure (aucune lecture du système), pour être testable et réutilisable dans
    Invoke-HyperVChecked.
  #>
  return @(
    "Cause la plus probable : l'appartenance au groupe Administrateurs Hyper-V"
    "(SID $script:HyperVAdminsSid) a été ajoutée sans réouverture de session. Le jeton ne"
    "prend une appartenance de groupe en compte qu'à l'OUVERTURE de la session Windows :"
    "fermer la session et la rouvrir (redémarrer le terminal ne suffit pas)."
  ) -join "`n"
}

function Get-HyperVAccessMessage {
  <#
  .SYNOPSIS
    Message d'échec quand la session ne peut pas piloter Hyper-V : il donne LES DEUX
    remèdes (relancer en administrateur, ou rejoindre le groupe Administrateurs Hyper-V).
  .DESCRIPTION
    Fonction pure : le nom d'utilisateur est passé en paramètre plutôt que lu dans
    l'environnement, pour que le message soit testable tel quel.
  .PARAMETER Reason
    Ce à quoi l'accès sert (affiché entre parenthèses).
  .PARAMETER UserName
    Compte à ajouter au groupe dans la commande proposée.
  #>
  param(
    [Parameter(Mandatory)][string]$Reason,
    [Parameter(Mandatory)][AllowEmptyString()][string]$UserName
  )
  $account = if ([string]::IsNullOrWhiteSpace($UserName)) { "<votre compte>" } else { $UserName }
  return @(
    "Accès à Hyper-V refusé ($Reason) : cette session n'est ni administrateur, ni membre"
    "du groupe Administrateurs Hyper-V (SID $script:HyperVAdminsSid)."
    ""
    "Deux remèdes, au choix :"
    "  1. Relancer ce script dans un PowerShell lancé en tant qu'administrateur ;"
    "  2. ou, une seule fois, depuis un PowerShell administrateur :"
    "       Add-LocalGroupMember -SID $script:HyperVAdminsSid -Member `"$account`""
    "     PUIS FERMER ET ROUVRIR LA SESSION WINDOWS. Le jeton ne prend l'appartenance"
    "     de groupe en compte qu'à l'ouverture de session : redémarrer le terminal ne"
    "     suffit pas."
  ) -join "`n"
}

function Assert-HyperVAccess {
  <#
  .SYNOPSIS
    Échoue avec le message de Get-HyperVAccessMessage si la session ne peut pas piloter
    Hyper-V (ni administrateur, ni membre du groupe Administrateurs Hyper-V).
  .PARAMETER Reason
    Ce à quoi l'accès sert (affiché dans le message).
  #>
  param([Parameter(Mandatory)][string]$Reason)
  if (-not (Test-HyperVOperator)) {
    throw (Get-HyperVAccessMessage -Reason $Reason -UserName $env:USERNAME)
  }
}

function Test-AccessDeniedError {
  <#
  .SYNOPSIS
    Vrai si l'erreur est un refus d'accès (E_ACCESSDENIED, 0x80070005), y compris
    enveloppée dans une ou plusieurs exceptions.
  .DESCRIPTION
    Reconnu par le HRESULT et par le type de l'exception, en remontant toute la chaîne
    InnerException. JAMAIS par le texte du message : il est traduit selon la langue de
    Windows, et s'y fier est le même piège que chercher un groupe par son nom.
    La catégorie PermissionDenied de l'ErrorRecord est une énumération, pas du texte :
    elle peut donc servir de signal supplémentaire.
  .PARAMETER ErrorRecord
    Un ErrorRecord (le `$_` d'un catch) ou directement une exception. $null accepté.
  #>
  param([Parameter(Mandatory)][AllowNull()]$ErrorRecord)
  if ($null -eq $ErrorRecord) { return $false }

  $accessDenied = -2147024891   # 0x80070005 (E_ACCESSDENIED), lu en Int32 signé.
  $exception = $ErrorRecord
  if ($ErrorRecord -is [System.Management.Automation.ErrorRecord]) {
    if ($ErrorRecord.CategoryInfo -and
        $ErrorRecord.CategoryInfo.Category -eq [System.Management.Automation.ErrorCategory]::PermissionDenied) {
      return $true
    }
    $exception = $ErrorRecord.Exception
  }

  $depth = 0
  while ($exception -and $depth -lt 16) {
    if ($exception -is [System.UnauthorizedAccessException] -or
        $exception -is [System.Security.SecurityException] -or
        $exception -is [System.Management.Automation.PSSecurityException]) {
      return $true
    }
    if ($exception.HResult -eq $accessDenied) { return $true }
    $exception = $exception.InnerException
    $depth++
  }
  return $false
}

function Invoke-HyperVChecked {
  <#
  .SYNOPSIS
    Exécute la PREMIÈRE commande Hyper-V d'un script en transformant un refus d'accès en
    message actionnable (Get-HyperVAccessHint).
  .DESCRIPTION
    Assert-HyperVAccess a déjà validé le jeton ; un refus d'accès à ce stade signifie
    presque toujours que l'appartenance au groupe a été ajoutée sans réouverture de
    session. Toute autre erreur est relancée telle quelle.
  .PARAMETER What
    Ce que la commande faisait (« inventaire des VM »), affiché en tête du message.
  .PARAMETER Script
    Le bloc à exécuter ; sa sortie est renvoyée telle quelle.
  #>
  param(
    [Parameter(Mandatory)][string]$What,
    [Parameter(Mandatory)][scriptblock]$Script
  )
  try {
    return (& $Script)
  } catch {
    if (Test-AccessDeniedError -ErrorRecord $_) {
      throw "$What : accès refusé par Hyper-V.`n$(Get-HyperVAccessHint)`n`nErreur d'origine : $($_.Exception.Message)"
    }
    throw
  }
}

function New-DebugKey {
  <#
  .SYNOPSIS
    Génère une clé de débogage noyau réseau (kdnet) : quatre groupes de neuf caractères
    en base 36 (0-9, a-z) séparés par des points, comme celles produites par kdnet.exe.
  #>
  $alphabet = "0123456789abcdefghijklmnopqrstuvwxyz".ToCharArray()
  $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
  $bytes = New-Object byte[] 36
  $rng.GetBytes($bytes)
  $chars = foreach ($b in $bytes) { $alphabet[$b % $alphabet.Length] }
  $groups = for ($i = 0; $i -lt 4; $i++) { -join $chars[($i * 9)..($i * 9 + 8)] }
  return ($groups -join ".")
}

function Test-DebugKey {
  <#
  .SYNOPSIS
    Vrai si la chaîne a le format d'une clé kdnet (quatre groupes alphanumériques
    séparés par des points).
  #>
  param([Parameter(Mandatory)][AllowEmptyString()][string]$Key)
  return $Key -match '^[0-9a-zA-Z]+\.[0-9a-zA-Z]+\.[0-9a-zA-Z]+\.[0-9a-zA-Z]+$'
}

function Invoke-NativeChecked {
  <#
  .SYNOPSIS
    Lance un exécutable natif (devgen, pnputil, bcdedit…) en capturant sa sortie, et
    échoue avec cette sortie si le code de retour n'est pas 0.
  .DESCRIPTION
    Windows PowerShell 5.1 transforme les lignes stderr d'un exécutable en erreurs
    terminantes quand `$ErrorActionPreference` vaut Stop et que `2>&1` est utilisé : la
    préférence est relâchée le temps de l'appel. Cette fonction est aussi envoyée telle
    quelle dans l'invité (voir Get-FunctionSource) par vm-prepare.ps1 et vm-cycle.ps1.
  .PARAMETER Exe
    Exécutable (nom dans le PATH ou chemin complet).
  .PARAMETER Arguments
    Arguments, un par élément.
  .OUTPUTS
    Les lignes de sortie (stdout et stderr) sous forme de chaînes.
  #>
  param(
    [Parameter(Mandatory)][string]$Exe,
    [AllowEmptyCollection()][string[]]$Arguments = @()
  )
  $previous = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    $global:LASTEXITCODE = 0
    $out = @(& $Exe @Arguments 2>&1 | ForEach-Object { "$_" })
    $code = $LASTEXITCODE
  } finally {
    $ErrorActionPreference = $previous
  }
  if ($code -ne 0) {
    throw "« $Exe $($Arguments -join ' ') » a échoué (code $code) :`n$($out -join "`n")"
  }
  return $out
}

function Get-FunctionSource {
  <#
  .SYNOPSIS
    Texte `function X { … }` d'une fonction du module, pour la redéfinir dans l'invité
    (Invoke-Command ne transporte pas les fonctions de l'hôte).
  #>
  param([Parameter(Mandatory)][string]$Name)
  $command = Get-Command -Name $Name -CommandType Function
  return "function $Name {`n$($command.Definition)`n}"
}

function ConvertFrom-DevgenAddOutput {
  <#
  .SYNOPSIS
    Extrait l'identifiant d'instance imprimé par `devgen /add` (« Device node created.
    Instance ID: SWD\DEVGEN\{…} »). Indépendant de la langue : on cherche un chemin
    d'instance SWD\ ou ROOT\ n'importe où dans la sortie.
  .OUTPUTS
    L'identifiant d'instance, ou $null s'il est introuvable.
  #>
  param([Parameter(Mandatory)][AllowEmptyCollection()][AllowEmptyString()][AllowNull()][string[]]$Output)
  foreach ($line in @($Output)) {
    if ($null -eq $line) { continue }
    if ($line -match '((?:SWD|ROOT)\\[^\s"]+)') { return $Matches[1] }
  }
  return $null
}

function Find-PublishedInf {
  <#
  .SYNOPSIS
    Retrouve le nom publié (oemN.inf) d'un paquet dans la sortie de
    `pnputil /enum-drivers`, à partir de son nom d'origine (conduit_kmd.inf).
  .DESCRIPTION
    La sortie est une suite de blocs « Étiquette : valeur » séparés par des lignes vides ;
    les étiquettes dépendent de la langue du système, pas les valeurs. Un bloc qui
    contient une valeur `oemN.inf` et une valeur égale au nom d'origine désigne le paquet.
  .OUTPUTS
    Les noms publiés trouvés (plusieurs si le paquet a été ajouté plusieurs fois).
  #>
  param(
    [Parameter(Mandatory)][AllowEmptyCollection()][AllowEmptyString()][AllowNull()][string[]]$Output,
    [Parameter(Mandatory)][string]$OriginalName
  )
  $found = @()
  $published = $null
  $isOriginal = $false
  foreach ($line in @($Output) + @("")) {
    if ($null -eq $line -or $line.Trim() -eq "") {
      if ($published -and $isOriginal) { $found += $published }
      $published = $null
      $isOriginal = $false
      continue
    }
    if ($line -match '^[^:]+:\s*(\S.*?)\s*$') {
      $value = $Matches[1]
      if ($value -match '^oem\d+\.inf$') { $published = $value }
      elseif ($value -ieq $OriginalName) { $isOriginal = $true }
    }
  }
  return $found
}

function Get-CycleSummary {
  <#
  .SYNOPSIS
    Résume une série de durées de cycle (secondes) : nombre, moyenne, minimum, maximum.
  #>
  param([Parameter(Mandatory)][AllowEmptyCollection()][double[]]$Durations)
  $n = @($Durations).Count
  if ($n -eq 0) {
    return [pscustomobject]@{ Count = 0; MeanSeconds = 0.0; MinSeconds = 0.0; MaxSeconds = 0.0 }
  }
  $stats = $Durations | Measure-Object -Average -Minimum -Maximum
  return [pscustomobject]@{
    Count       = $n
    MeanSeconds = [math]::Round($stats.Average, 2)
    MinSeconds  = [math]::Round($stats.Minimum, 2)
    MaxSeconds  = [math]::Round($stats.Maximum, 2)
  }
}

function Get-WdkRoot {
  <#
  .SYNOPSIS
    Racine du WDK (KitsRoot10) lue dans le registre, comme le fait wdk-build.
  #>
  $key = "HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows Kits\Installed Roots"
  $root = (Get-ItemProperty -Path $key -Name KitsRoot10).KitsRoot10
  if (-not (Test-Path $root)) { throw "WDK introuvable : $root (clé KitsRoot10)" }
  return $root
}

function Get-DevgenPath {
  <#
  .SYNOPSIS
    Chemin de devgen.exe (WDK 26100, Tools\<version>\x64), à copier dans la VM.
  .PARAMETER WdkVersion
    Version du WDK (packaging\windows\versions.json, champ `wdk`).
  #>
  param([Parameter(Mandatory)][string]$WdkVersion)
  $path = Join-Path (Get-WdkRoot) "Tools\$WdkVersion\x64\devgen.exe"
  if (-not (Test-Path $path)) { throw "devgen.exe introuvable : $path" }
  return $path
}

function Get-DefaultSwitchHostIp {
  <#
  .SYNOPSIS
    Adresse IPv4 de l'hôte sur le commutateur « Default Switch » (celle que la VM joint
    pour le débogueur noyau réseau).
  #>
  $addr = Get-NetIPAddress -AddressFamily IPv4 |
    Where-Object { $_.InterfaceAlias -like "vEthernet (Default Switch)*" } |
    Select-Object -First 1
  if (-not $addr) { throw "Aucune adresse IPv4 sur « vEthernet (Default Switch) » : Hyper-V est-il actif ?" }
  return $addr.IPAddress
}

function Wait-VMHeartbeat {
  <#
  .SYNOPSIS
    Attend que l'invité réponde au service Heartbeat d'Hyper-V (VM démarrée et système
    chargé), avec un délai borné.
  #>
  param(
    [Parameter(Mandatory)][string]$Name,
    [int]$TimeoutSeconds = 600
  )
  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    $vm = Get-VM -Name $Name
    if ($vm.State -eq "Running" -and $vm.Heartbeat -like "Ok*") { return }
    Start-Sleep -Seconds 5
  }
  throw "La VM « $Name » ne répond pas (Heartbeat) après $TimeoutSeconds s."
}

function Wait-VMReboot {
  <#
  .SYNOPSIS
    Après un `shutdown /r` dans l'invité : attend que le Heartbeat disparaisse (arrêt
    effectif, au plus deux minutes) puis revienne (système rechargé), avec délai borné.
  #>
  param(
    [Parameter(Mandatory)][string]$Name,
    [int]$TimeoutSeconds = 600
  )
  $deadline = (Get-Date).AddSeconds(120)
  while ((Get-Date) -lt $deadline) {
    $vm = Get-VM -Name $Name
    if ($vm.State -ne "Running" -or $vm.Heartbeat -notlike "Ok*") { break }
    Start-Sleep -Seconds 2
  }
  Wait-VMHeartbeat -Name $Name -TimeoutSeconds $TimeoutSeconds
}

function New-GuestSession {
  <#
  .SYNOPSIS
    Ouvre une session PowerShell Direct vers l'invité, en réessayant jusqu'à ce que le
    service soit prêt (après un redémarrage par exemple).
  #>
  param(
    [Parameter(Mandatory)][string]$Name,
    [Parameter(Mandatory)][pscredential]$Credential,
    [int]$TimeoutSeconds = 600
  )
  Wait-VMHeartbeat -Name $Name -TimeoutSeconds $TimeoutSeconds
  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  $lastError = $null
  while ((Get-Date) -lt $deadline) {
    try {
      return New-PSSession -VMName $Name -Credential $Credential -ErrorAction Stop
    } catch {
      $lastError = $_
      Start-Sleep -Seconds 5
    }
  }
  throw "Impossible d'ouvrir une session PowerShell Direct vers « $Name » : $lastError"
}

Export-ModuleMember -Function Test-Elevated, Test-SidPresent, Get-TokenSid,
  Test-HyperVOperator, Get-HyperVAccessHint, Get-HyperVAccessMessage, Assert-HyperVAccess,
  Test-AccessDeniedError, Invoke-HyperVChecked, New-DebugKey, Test-DebugKey,
  Invoke-NativeChecked, Get-FunctionSource, ConvertFrom-DevgenAddOutput, Find-PublishedInf,
  Get-CycleSummary, Get-WdkRoot, Get-DevgenPath, Get-DefaultSwitchHostIp, Wait-VMHeartbeat,
  Wait-VMReboot, New-GuestSession
