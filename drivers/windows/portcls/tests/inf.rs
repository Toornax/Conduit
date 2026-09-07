//! Cohérence entre `conduit-kmd/conduit_kmd.inx` et les constantes Rust (M1a-09, M1b-02,
//! driver-design.md §4.1 et §4.2).
//!
//! L'INF et le pilote se donnent rendez-vous sur des chaînes et des GUID qu'aucun
//! compilateur ne rapproche :
//!
//! | Dans l'INF | Dans le pilote | Symptôme d'une divergence |
//! |---|---|---|
//! | nom de référence d'un `AddInterface` | `WAVE_RENDER_NAMES[n]`… passé à `PcRegisterSubdevice` | le périphérique s'installe, **aucun endpoint** n'apparaît |
//! | `GUID.PinName.Cable<n>` | `pin_name_guid(n)` = `KsPinDescriptor.Name` | l'endpoint s'appelle « Haut-parleurs » / « Ligne » |
//! | `%KSCATEGORY_*%` | `portcls_sys::KSCATEGORY_*` (en-têtes du WDK) | interface publiée dans la mauvaise classe |
//! | `HKR,,ReserveSize`… (`.HW`) | `conduit_kmd_core::params` (M1b-01) | le poste est réglé sur une valeur que le pilote ne tient pas pour son défaut |
//!
//! Ces pannes ne se voient que dans une VM, et mal. Ce test les transforme en échec de
//! `cargo test -p portcls`, donc de `tools/check.ps1` et de la CI.
//!
//! # Le fichier est **engendré**
//!
//! À seize câbles, l'INF fait près de quatre cents lignes dont la quasi-totalité est
//! répétitive : dix `AddInterface`, huit sections d'interface, un `HKR\MediaCategories` et
//! quatorze jetons `[Strings]` **par câble**. Les écrire à la main, c'est écrire seize fois
//! la même faute de frappe muette.
//!
//! [`inf_attendu`] engendre donc le fichier entier à partir de `portcls::CABLE_COUNT` et
//! des constantes du pilote, sur la convention de `layout.golden` (engendré, commité,
//! fraîcheur vérifiée) — mais **sans** le détour par PowerShell, qui n'existe là que pour
//! appeler `cl.exe` : la source de vérité (`pin_name_guid`, les noms de sous-périphérique,
//! les GUID de catégorie) est en Rust, et ces tests sont `std`.
//!
//! - [`inx_est_a_jour`] compare le fichier commité au texte engendré ;
//! - [`regenerer_inx`] (`#[ignore]`) le réécrit.
//!
//! `tools/check.ps1` lance déjà `cargo test -p portcls` : la fraîcheur est vérifiée en
//! intégration continue sans rien ajouter.
//!
//! # Encodage
//!
//! Un INF non ASCII doit être en UTF-16 LE (« General Guidelines for INF Files »), avec
//! BOM et fins de ligne CRLF ; sans le BOM, `stampinf` recopierait de l'UTF-8 que SetupAPI
//! relirait en page de codes ANSI (« câbles » → « cÃ¢bles »). `.gitattributes`
//! (`working-tree-encoding=UTF-16LE-BOM`) l'obtient au *checkout*, mais ne répare pas un
//! fichier réécrit par un tiers : [`octets_utf16le`] l'écrit correctement, sur le modèle de
//! `conduitd::autostart::xml::utf16le`.
//!
//! La **comparaison**, elle, se fait sur le texte décodé et sur des fins de ligne
//! normalisées : comparer des octets bruts ferait osciller le résultat avec la
//! configuration git de la machine.
//!
//! Le fichier n'emploie que des commentaires en début de ligne : l'analyse ci-dessous les
//! retire sans se soucier des `;` entre guillemets (le descripteur SDDL en contient).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use conduit_kmd_core::params::Param;
use portcls::{
    CABLE_COUNT, TOPO_CAPTURE_NAMES, TOPO_RENDER_NAMES, WAVE_CAPTURE_NAMES, WAVE_RENDER_NAMES,
    pin_name_guid,
};
use portcls_sys::{
    GUID, KSCATEGORY_AUDIO, KSCATEGORY_CAPTURE, KSCATEGORY_REALTIME, KSCATEGORY_RENDER,
    KSCATEGORY_TOPOLOGY,
};

/// CLSID du proxy Kernel Streaming (ksproxy.ax), tel que `%SystemRoot%\INF\ks.inf` le
/// définit (`Proxy.CLSID`) et que tout INF d'adaptateur audio l'écrit sous `HKR,,CLSID`.
/// Il ne sort pas des en-têtes du WDK : la valeur attendue est écrite ici.
const PROXY_CLSID: &str = "{17CCA71B-ECD7-11D0-B908-00A0C9223196}";

/// Section `[…NT.Interfaces]` de l'INF.
const SECTION_INTERFACES: &str = "ConduitCable_Install.NT.Interfaces";

/// Les quatre sous-périphériques d'un câble, **dans l'ordre où l'INF les publie** :
/// préfixe du nom de référence, catégories KS de son `AddInterface`, rôle en clair (pour
/// le `FriendlyName` du filtre).
///
/// Le tableau reflète ce que PortCls enregistre — `CLSID_PortWaveRT` pour les filtres wave
/// (audio, rendu ou capture, temps réel), `CLSID_PortTopology` pour les filtres topologie
/// (audio, topologie) — et l'INF doit le publier exactement.
const SOUS_PERIPHERIQUES: &[(&str, &[&str], &str)] = &[
    (
        "WaveRender",
        &[
            "KSCATEGORY_AUDIO",
            "KSCATEGORY_RENDER",
            "KSCATEGORY_REALTIME",
        ],
        "rendu, WaveRT",
    ),
    (
        "TopoRender",
        &["KSCATEGORY_AUDIO", "KSCATEGORY_TOPOLOGY"],
        "rendu, topologie",
    ),
    (
        "WaveCapture",
        &[
            "KSCATEGORY_AUDIO",
            "KSCATEGORY_CAPTURE",
            "KSCATEGORY_REALTIME",
        ],
        "capture, WaveRT",
    ),
    (
        "TopoCapture",
        &["KSCATEGORY_AUDIO", "KSCATEGORY_TOPOLOGY"],
        "capture, topologie",
    ),
];

// -------------------------------------------------------------------------------------
// Lecture et analyse de l'INX commité.
// -------------------------------------------------------------------------------------

/// Chemin de l'INX, relatif au crate `portcls`.
fn chemin_inx() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../conduit-kmd/conduit_kmd.inx")
}

/// L'INX décodé, BOM UTF-16 LE exigé.
fn lire_inx() -> String {
    let chemin = chemin_inx();
    let octets =
        std::fs::read(&chemin).unwrap_or_else(|e| panic!("lecture de {} : {e}", chemin.display()));
    assert!(
        octets.len() >= 2 && octets[0] == 0xFF && octets[1] == 0xFE,
        "{} n'a pas le BOM UTF-16 LE attendu (premiers octets : {:02X?}).\n\
         Un INF contenant des caractères non ASCII doit être en UTF-16 LE. Le dépôt le \n\
         garde en UTF-8 et .gitattributes le restitue en UTF-16 LE \n\
         (« *.inx text working-tree-encoding=UTF-16LE-BOM eol=crlf ») : si git est trop \n\
         ancien ou si un éditeur a réécrit le fichier, rétablissez-le par \n\
         « git checkout -- drivers/windows/conduit-kmd/conduit_kmd.inx ».",
        chemin.display(),
        &octets[..octets.len().min(4)]
    );
    let unites: Vec<u16> = octets[2..]
        .chunks_exact(2)
        .map(|paire| u16::from_le_bytes([paire[0], paire[1]]))
        .collect();
    String::from_utf16(&unites).expect("INX : UTF-16 LE invalide")
}

/// Les lignes utiles : commentaires de début de ligne et lignes vides retirés.
fn lignes(inx: &str) -> Vec<&str> {
    inx.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with(';'))
        .collect()
}

/// Les noms de section (`[Xxx]`) présents.
fn sections(inx: &str) -> BTreeSet<String> {
    lignes(inx)
        .iter()
        .filter_map(|l| l.strip_prefix('[')?.strip_suffix(']'))
        .map(str::to_owned)
        .collect()
}

/// Les lignes d'une section, dans l'ordre.
fn section<'a>(inx: &'a str, nom: &str) -> Vec<&'a str> {
    let entete = format!("[{nom}]");
    lignes(inx)
        .into_iter()
        .skip_while(|l| *l != entete)
        .skip(1)
        .take_while(|l| !l.starts_with('['))
        .collect()
}

/// La table `[Strings]` : jeton → valeur, guillemets retirés.
fn strings(inx: &str) -> BTreeMap<String, String> {
    section(inx, "Strings")
        .into_iter()
        .filter_map(|ligne| {
            let (cle, valeur) = ligne.split_once('=')?;
            let valeur = valeur.trim();
            let valeur = valeur
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(valeur);
            Some((cle.trim().to_owned(), valeur.to_owned()))
        })
        .collect()
}

/// Tous les jetons `%Xxx%` référencés hors commentaires. Les DIRID numériques (`%13%`)
/// n'appartiennent pas à `[Strings]` et sont écartés.
fn jetons_utilises(inx: &str) -> BTreeSet<String> {
    let mut jetons = BTreeSet::new();
    let corps: Vec<&str> = lignes(inx)
        .into_iter()
        .take_while(|l| *l != "[Strings]")
        .collect();
    for ligne in corps {
        let mut morceaux = ligne.split('%');
        // Hors des `%`, un morceau sur deux : le premier est avant le premier `%`.
        let _ = morceaux.next();
        let mut dedans = true;
        for morceau in morceaux {
            if dedans && !morceau.is_empty() && !morceau.chars().all(|c| c.is_ascii_digit()) {
                jetons.insert(morceau.to_owned());
            }
            dedans = !dedans;
        }
    }
    jetons
}

/// Un GUID au format des `[Strings]` d'un INF : `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`.
fn guid_texte(guid: &GUID) -> String {
    let d = guid.Data4;
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        guid.Data1, guid.Data2, guid.Data3, d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7]
    )
}

/// Une constante de nom de sous-périphérique, en texte : coupée au **premier** NUL.
///
/// Les gabarits de `portcls::utf16z_numbered` sont remplis de zéros pour que
/// « WaveRender0 » et « WaveRender15 » partagent un type ; `split_last` n'en verrait que
/// le dernier.
fn nom_utf16(nom: &[u16]) -> String {
    assert_eq!(nom.last(), Some(&0), "nom de sous-périphérique sans NUL");
    let unites: Vec<u16> = nom.iter().copied().take_while(|u| *u != 0).collect();
    String::from_utf16(&unites).expect("nom de sous-périphérique non UTF-16")
}

/// Les quatre noms de sous-périphérique du câble `cable`, dans l'ordre de
/// [`SOUS_PERIPHERIQUES`], tels que le pilote les passe à `PcRegisterSubdevice`.
fn noms_du_cable(cable: usize) -> [String; 4] {
    [
        nom_utf16(&WAVE_RENDER_NAMES[cable]),
        nom_utf16(&TOPO_RENDER_NAMES[cable]),
        nom_utf16(&WAVE_CAPTURE_NAMES[cable]),
        nom_utf16(&TOPO_CAPTURE_NAMES[cable]),
    ]
}

/// Les `AddInterface` attendus, **câble par câble** : (jeton de catégorie, jeton de nom,
/// section). L'ordre est celui du fichier : les quatre sous-périphériques d'un câble se
/// suivent, et non les catégories.
fn interfaces_attendues() -> Vec<(String, String, String)> {
    let mut attendues = Vec::new();
    for cable in 0..CABLE_COUNT {
        for (famille, categories, _) in SOUS_PERIPHERIQUES {
            for categorie in *categories {
                attendues.push((
                    (*categorie).to_owned(),
                    format!("KSNAME_{famille}{cable}"),
                    format!("{famille}{cable}.Interface"),
                ));
            }
        }
    }
    attendues
}

/// Les lignes attendues de `[ConduitCable_PinNames_AddReg]`, une par câble.
fn media_categories_attendues() -> Vec<String> {
    (0..CABLE_COUNT)
        .map(|cable| {
            format!(
                "HKR,%MediaCategories%\\%GUID.PinName.Cable{cable}%,Name,,%PinName.Cable{cable}%"
            )
        })
        .collect()
}

// -------------------------------------------------------------------------------------
// Le générateur.
//
// Les blocs invariants (en-tête, [Version], services, sécurité) sont des littéraux ; tout
// ce qui se répète par câble est engendré à partir de `portcls::CABLE_COUNT` et des
// constantes du pilote.
// -------------------------------------------------------------------------------------

/// En-tête de commentaires : rôle du fichier, encodage, jalons.
const ENTETE: &str = r#"
;
; conduit_kmd.inx : source de l'INF du pilote Conduit (câbles audio virtuels).
;
; ENGENDRÉ par `portcls/tests/inf.rs` (fonction `inf_attendu`) : n'éditez pas ce fichier
; à la main. Les blocs par câble — AddInterface, sections d'interface, HKR\MediaCategories
; et jetons [Strings] — sont dérivés de `portcls::CABLE_COUNT` et des constantes du
; pilote. Pour le régénérer :
;   cargo test -p portcls --test inf -- --ignored regenerer_inx
; puis commitez. `cargo test -p portcls` (donc tools\check.ps1 et la CI) échoue tant que
; le fichier commité diffère du texte engendré.
;
; ENCODAGE : UTF-16 LE avec BOM dans la copie de travail. Un INF qui contient des
; caractères non ASCII doit être en UTF-16 LE (« General Guidelines for INF Files » :
; « if your INF contains non-ASCII characters, you must save the file as a Unicode
; (UTF-16 LE) file ») ; en UTF-8 sans BOM, SetupAPI le relit dans la page de codes ANSI
; et « câbles » deviendrait « cÃ¢bles » dans le gestionnaire de périphériques.
; `.gitattributes` garde le fichier en UTF-8 dans le dépôt (diffs lisibles) et le
; restitue en UTF-16 LE : `*.inx text working-tree-encoding=UTF-16LE-BOM eol=crlf`.
; Le test `portcls/tests/inf.rs` échoue si le BOM manque dans la copie de travail
; (git trop ancien, fichier réécrit par un éditeur) : `git checkout -- <fichier>` répare.
;
; `cargo wdk build` copie ce fichier en conduit_kmd.inf, remplace DriverVer et $ARCH$
; (stampinf, qui normalise aussi les fins de ligne en CRLF), génère le catalogue
; (inf2cat), signe avec le certificat de test et vérifie avec `infverif /w` (exigences
; « Windows Driver » : fichiers dans le magasin des pilotes, DIRID 13, écritures de
; registre limitées aux clés du périphérique). Le nom du fichier est imposé par
; cargo-wdk : nom du crate avec `_`.
;
; M1a-02 : adaptateur PortCls pour un périphérique énuméré à la racine (Root\ConduitCable),
; classe MEDIA. Include/Needs de ks.inf et wdmaudio.inf enregistrent Kernel Streaming et
; PortCls ; AssociatedFilters et Driver sont les valeurs que wdmaud attend d'un adaptateur
; audio.
; M1a-06 : interfaces KS des sous-périphériques que StartDevice enregistre
; (docs/driver-design.md §4.1), quatre par câble : WaveRender<n> (KSCATEGORY_AUDIO,
; RENDER, REALTIME), WaveCapture<n> (AUDIO, CAPTURE, REALTIME), TopoRender<n> et
; TopoCapture<n> (AUDIO, TOPOLOGY).
; Le nom de référence de chaque AddInterface est le nom passé à PcRegisterSubdevice ;
; CLSID est le proxy KS (ksproxy).
; M1a-09 : nom des endpoints (docs/driver-design.md §4.2). Le nom affiché par Windows
; est composé du **nom de la broche bridge** et du nom de l'adaptateur ; la broche se
; nomme par son GUID `KsPinDescriptor.Name`, que ce fichier associe à « Conduit <n+1> »
; dans `HKR\MediaCategories`. Le GUID est celui de `portcls::adapter::pin_name_guid(n)` et
; `portcls/tests/inf.rs` vérifie l'égalité. Sécurité de l'objet de périphérique et
; DeviceType dans .NT.HW.
; M1b-02 : un câble devient seize. Rien de nouveau n'est déclaré ici : les mêmes blocs,
; répétés par numéro de câble.
; Structure de référence : SYSVAD SimpleAudioSample, jamais copié.
;
; Désinstallation (F-52) : aucune section DelReg ni DelFiles n'est nécessaire. Toutes
; les écritures de ce fichier sont des HKR — clé logicielle du périphérique
; (ConduitCable_*_AddReg, sections .Interfaces) ou clé matérielle (.NT.HW) — que PnP
; supprime avec le périphérique, et les fichiers vivent dans le magasin des pilotes
; (DIRID 13, PnpLockdown=1), retirés par `pnputil /delete-driver`. Une section DelReg
; serait d'ailleurs refusée par infverif /w si elle visait une clé hors du périphérique.
;
"#;

/// De `[Version]` à `[ConduitCable_AddReg]` : rien n'y dépend du nombre de câbles.
const SECTIONS_FIXES: &str = r#"
[Version]
Signature   = "$WINDOWS NT$"
Class       = MEDIA
ClassGuid   = {4d36e96c-e325-11ce-bfc1-08002be10318}
Provider    = %Conduit%
CatalogFile = conduit_kmd.cat
DriverVer   =
PnpLockdown = 1

[DestinationDirs]
DefaultDestDir = 13

[SourceDisksNames]
1 = %DiskName%,,,""

[SourceDisksFiles]
conduit_kmd.sys = 1

[Manufacturer]
%Conduit% = Conduit,NT$ARCH$.10.0...16299

[Conduit.NT$ARCH$.10.0...16299]
%ConduitCable.DeviceDesc% = ConduitCable_Install, Root\ConduitCable

[ConduitCable_Install.NT]
Include   = ks.inf, wdmaudio.inf
Needs     = KS.Registration, WDMAUDIO.Registration
CopyFiles = ConduitCable_CopyFiles
AddReg    = ConduitCable_AddReg, ConduitCable_PinNames_AddReg

[ConduitCable_CopyFiles]
conduit_kmd.sys

[ConduitCable_AddReg]
HKR,,AssociatedFilters,,"wdmaud,swmidi,redbook"
HKR,,Driver,,conduit_kmd.sys
"#;

/// Commentaire et en-tête de `[ConduitCable_PinNames_AddReg]`.
const ENTETE_NOMS_DE_BROCHE: &str = r#"
; Noms de broche (§4.2). KS répond à KSPROPERTY_PIN_NAME en cherchant d'abord une chaîne
; pour le GUID `KsPinDescriptor.Name` de la broche, puis, à défaut, pour son GUID
; `KsPinDescriptor.Category`. Depuis Windows 10 1809 la recherche commence par la clé
; **logicielle du périphérique** (HKR\MediaCategories), remplie par cette section : c'est
; le seul levier documenté pour nommer un endpoint sans toucher à l'espace de noms global
; (HKLM\SYSTEM\CurrentControlSet\Control\MediaCategories, réservé et interdit par
; infverif /w). Sans elle, la catégorie KSNODETYPE_SPEAKER donnerait « Haut-parleurs » et
; KSNODETYPE_LINE_CONNECTOR « Ligne ». Une ligne par câble.
[ConduitCable_PinNames_AddReg]
"#;

/// Service et clé matérielle : indépendants du nombre de câbles (un seul périphérique
/// PnP porte les seize).
const SERVICE_ET_SECURITE: &str = r#"
[ConduitCable_Install.NT.Services]
AddService = conduit_kmd, 0x00000002, ConduitCable_Service

[ConduitCable_Service]
DisplayName   = %ConduitCable.ServiceDesc%
ServiceType   = 1
StartType     = 3
ErrorControl  = 1
ServiceBinary = %13%\conduit_kmd.sys

; Objet de périphérique créé par PortCls : type FILE_DEVICE_SOUND et descripteur de
; sécurité standard des adaptateurs audio (SDDL_DEVOBJ_SYS_ALL_ADM_RWX_WORLD_RWX_RES_RWX,
; comme SYSVAD). Le service audio (audiodg.exe s'exécute sous LOCAL SERVICE) doit pouvoir
; ouvrir les filtres KS ; le descripteur par défaut d'un périphérique énuméré à la racine
; ne l'accorde pas, et l'échec se lit seulement comme « aucun endpoint ».
[ConduitCable_Install.NT.HW]
AddReg = ConduitCable_HW_AddReg

[ConduitCable_HW_AddReg]
HKR,,DeviceType,0x10001,0x0000001D
HKR,,Security,,"D:P(A;;GA;;;SY)(A;;GRGWGX;;;BA)(A;;GRGWGX;;;WD)(A;;GRGWGX;;;RC)"
"#;

/// Commentaire des valeurs de paramètres, qui prolongent `[ConduitCable_HW_AddReg]`.
const ENTETE_PARAMETRES: &str = r#"
; Paramètres du pilote (M1b-01). `conduit_kmd::registry` les lit au démarrage par
; IoOpenDeviceRegistryKey(PLUGPLAY_REGKEY_DEVICE), qui ouvre exactement la clé matérielle
; que cette section alimente. 0x10001 = FLG_ADDREG_TYPE_DWORD.
;
; Les trois défauts sont ENGENDRÉS depuis conduit_kmd_core::params, par
; Param::default_value. Les modifier ici ne servirait à rien : le pilote garde les siens,
; et le test defauts_de_parametres_identiques_au_pilote signale la divergence.
;
; Aucune de ces valeurs n'est nécessaire au chargement : absente, d'un autre type ou hors
; bornes, elle est remplacée par le défaut et consignée au journal d'événements. Elles
; sont écrites pour que `regedit` montre à l'administrateur ce qu'il peut régler.
"#;

/// Commentaire et en-tête de `[…NT.Interfaces]`.
const ENTETE_INTERFACES: &str = r#"
; Interfaces KS des sous-périphériques, câble par câble. Le deuxième champ est le nom de
; référence du sous-périphérique, tel que PcRegisterSubdevice le reçoit : il doit
; correspondre exactement aux constantes de `portcls::adapter`
; (WAVE_RENDER_NAMES[n], TOPO_RENDER_NAMES[n], WAVE_CAPTURE_NAMES[n],
; TOPO_CAPTURE_NAMES[n]). Une divergence donne un périphérique qui s'installe sans publier
; aucun endpoint ; `portcls/tests/inf.rs` la détecte.
[ConduitCable_Install.NT.Interfaces]
"#;

/// Commentaire des sections `[…<n>.Interface]`.
const ENTETE_FRIENDLY_NAMES: &str = r#"
; FriendlyName par interface : nom du **filtre** dans le registre des interfaces de
; périphérique, pas celui de l'endpoint (§4.2). Il sert au diagnostic (Get-PnpDevice
; -Class MEDIA, regedit) : chacun nomme le câble et le rôle du filtre.
"#;

/// Les lignes `HKR` des trois paramètres, avec leur valeur par défaut.
///
/// Source unique : `conduit_kmd_core::params`, le module que le pilote consulte pour
/// valider ce qu'il lit. Un défaut de l'INF différent d'un défaut du pilote donnerait un
/// poste réglé sur une valeur que le code ne considère jamais comme « la valeur par
/// défaut » — une divergence qu'aucun symptôme ne trahirait.
fn parametres_attendus() -> Vec<String> {
    Param::ALL
        .iter()
        .map(|param| {
            format!(
                "HKR,,{},0x10001,{}",
                param.value_name(),
                param.default_value()
            )
        })
        .collect()
}

/// Les lignes d'un bloc littéral, sans le saut de ligne de mise en page initial.
fn bloc(texte: &str) -> impl Iterator<Item = &str> {
    texte.strip_prefix('\n').unwrap_or(texte).lines()
}

/// Un bloc de `[Strings]` : clés alignées sur la plus longue (au moins `colonne`), puis
/// « = "valeur" ».
fn bloc_strings(entrees: &[(String, String)], colonne: usize) -> Vec<String> {
    let largeur = entrees
        .iter()
        .map(|(cle, _)| cle.len())
        .max()
        .unwrap_or(0)
        .max(colonne);
    entrees
        .iter()
        .map(|(cle, valeur)| format!("{cle:largeur$} = \"{valeur}\""))
        .collect()
}

/// Les lignes `AddInterface`, colonnes alignées sur le plus long jeton de chaque champ.
fn bloc_add_interface(interfaces: &[(String, String, String)]) -> Vec<String> {
    let jeton = |texte: &str| format!("%{texte}%,");
    let largeur =
        |champs: &mut dyn Iterator<Item = String>| champs.map(|c| c.len()).max().unwrap_or(0) + 1;
    let cat = largeur(&mut interfaces.iter().map(|(c, _, _)| jeton(c)));
    let nom = largeur(&mut interfaces.iter().map(|(_, n, _)| jeton(n)));
    interfaces
        .iter()
        .map(|(categorie, reference, section)| {
            let categorie = jeton(categorie);
            let reference = jeton(reference);
            format!("AddInterface = {categorie:cat$}{reference:nom$}{section}")
        })
        .collect()
}

/// Le texte attendu de `conduit_kmd.inx`, fins de ligne `\n` (l'écriture les convertit en
/// CRLF).
fn inf_attendu() -> String {
    let mut inf: Vec<String> = Vec::new();
    let mut ajouter = |lignes: &mut dyn Iterator<Item = String>| inf.extend(lignes);

    ajouter(&mut bloc(ENTETE).map(str::to_owned));
    ajouter(&mut std::iter::once(String::new()));
    ajouter(&mut bloc(SECTIONS_FIXES).map(str::to_owned));
    ajouter(&mut std::iter::once(String::new()));

    // Un `HKR\MediaCategories` par câble.
    ajouter(&mut bloc(ENTETE_NOMS_DE_BROCHE).map(str::to_owned));
    ajouter(&mut media_categories_attendues().into_iter());
    ajouter(&mut std::iter::once(String::new()));

    ajouter(&mut bloc(SERVICE_ET_SECURITE).map(str::to_owned));

    // Trois `HKR` de plus dans la même section : les défauts des paramètres (M1b-01).
    ajouter(&mut std::iter::once(String::new()));
    ajouter(&mut bloc(ENTETE_PARAMETRES).map(str::to_owned));
    ajouter(&mut parametres_attendus().into_iter());
    ajouter(&mut std::iter::once(String::new()));

    // Dix `AddInterface` par câble, groupés par câble.
    ajouter(&mut bloc(ENTETE_INTERFACES).map(str::to_owned));
    ajouter(&mut bloc_add_interface(&interfaces_attendues()).into_iter());
    ajouter(&mut std::iter::once(String::new()));

    // Deux sections par sous-périphérique, soit huit par câble.
    ajouter(&mut bloc(ENTETE_FRIENDLY_NAMES).map(str::to_owned));
    for cable in 0..CABLE_COUNT {
        for (famille, _, _) in SOUS_PERIPHERIQUES {
            let nom = format!("{famille}{cable}");
            ajouter(
                &mut [
                    format!("[{nom}.Interface]"),
                    format!("AddReg = {nom}.Interface.AddReg"),
                    String::new(),
                    format!("[{nom}.Interface.AddReg]"),
                    "HKR,,CLSID,,%Proxy.CLSID%".to_owned(),
                    format!("HKR,,FriendlyName,,%Conduit.{nom}.FriendlyName%"),
                    String::new(),
                ]
                .into_iter(),
            );
        }
    }

    ajouter(&mut std::iter::once("[Strings]".to_owned()));
    ajouter(
        &mut bloc_strings(
            &[
                ("Conduit".to_owned(), "Conduit".to_owned()),
                (
                    "DiskName".to_owned(),
                    "Disque d'installation Conduit".to_owned(),
                ),
                (
                    "ConduitCable.DeviceDesc".to_owned(),
                    "Conduit — câbles audio virtuels".to_owned(),
                ),
                (
                    "ConduitCable.ServiceDesc".to_owned(),
                    "Conduit — pilote de câbles audio virtuels".to_owned(),
                ),
            ],
            0,
        )
        .into_iter(),
    );

    // Noms de sous-périphérique : quatre jetons par câble, lus dans les constantes.
    let mut noms = Vec::new();
    for cable in 0..CABLE_COUNT {
        for ((famille, _, _), nom) in SOUS_PERIPHERIQUES.iter().zip(noms_du_cable(cable)) {
            noms.push((format!("KSNAME_{famille}{cable}"), nom));
        }
    }
    ajouter(&mut std::iter::once(String::new()));
    ajouter(
        &mut bloc(
            "\n; Noms de sous-périphérique : identiques aux constantes de `portcls::adapter`,\n\
             ; quatre par câble.\n",
        )
        .map(str::to_owned),
    );
    ajouter(&mut bloc_strings(&noms, 0).into_iter());

    // FriendlyName de chaque filtre : quatre par câble.
    let mut amicaux = Vec::new();
    for cable in 0..CABLE_COUNT {
        for (famille, _, role) in SOUS_PERIPHERIQUES {
            amicaux.push((
                format!("Conduit.{famille}{cable}.FriendlyName"),
                format!("Conduit {} ({role})", cable + 1),
            ));
        }
    }
    ajouter(&mut std::iter::once(String::new()));
    ajouter(
        &mut bloc("\n; Noms des filtres dans le registre des interfaces (diagnostic).\n")
            .map(str::to_owned),
    );
    ajouter(&mut bloc_strings(&amicaux, 0).into_iter());

    // Noms de broche : le GUID et la chaîne affichée, par câble.
    let mut broches = vec![("MediaCategories".to_owned(), "MediaCategories".to_owned())];
    for cable in 0..CABLE_COUNT {
        let numero = u8::try_from(cable).expect("numéro de câble sur un octet");
        broches.push((
            format!("GUID.PinName.Cable{cable}"),
            guid_texte(&pin_name_guid(numero)),
        ));
        broches.push((
            format!("PinName.Cable{cable}"),
            format!("Conduit {}", cable + 1),
        ));
    }
    ajouter(&mut std::iter::once(String::new()));
    ajouter(
        &mut bloc(
            "\n; Noms de broche : c'est eux que l'utilisateur voit (§4.2). Le GUID est celui de\n\
             ; `portcls::adapter::pin_name_guid(n)` ; seul son dernier octet porte le numéro du\n\
             ; câble, ce qui donne les seize. Sous-clé « MediaCategories » de la clé logicielle du\n\
             ; périphérique (HKR), pas l'espace global HKLM.\n",
        )
        .map(str::to_owned),
    );
    // Colonne 22 : celle du fichier d'origine, assez large pour `GUID.PinName.Cable15`.
    ajouter(&mut bloc_strings(&broches, 22).into_iter());

    // Catégories KS : les cinq GUID des en-têtes du WDK, et le proxy KS.
    let mut categories: Vec<(String, String)> = [
        ("KSCATEGORY_AUDIO", KSCATEGORY_AUDIO),
        ("KSCATEGORY_RENDER", KSCATEGORY_RENDER),
        ("KSCATEGORY_CAPTURE", KSCATEGORY_CAPTURE),
        ("KSCATEGORY_REALTIME", KSCATEGORY_REALTIME),
        ("KSCATEGORY_TOPOLOGY", KSCATEGORY_TOPOLOGY),
    ]
    .into_iter()
    .map(|(jeton, guid)| (jeton.to_owned(), guid_texte(&guid)))
    .collect();
    categories.push(("Proxy.CLSID".to_owned(), PROXY_CLSID.to_owned()));
    ajouter(&mut std::iter::once(String::new()));
    ajouter(
        &mut bloc(
            "\n; Catégories KS (ksmedia.h) et proxy KS (ksproxy.ax, %SystemRoot%\\INF\\ks.inf).\n",
        )
        .map(str::to_owned),
    );
    ajouter(&mut bloc_strings(&categories, 0).into_iter());

    // `ajouter` n'est plus appelée : son emprunt de `inf` s'arrête ici.
    let mut texte = inf.join("\n");
    texte.push('\n');
    texte
}

/// Le texte en UTF-16 LE avec BOM et fins de ligne CRLF, tel qu'un INF doit être écrit.
///
/// Même problème et même solution que `conduitd::autostart::xml::utf16le` : le contenu
/// doit *être* de l'UTF-16, pas seulement s'en réclamer.
fn octets_utf16le(texte: &str) -> Vec<u8> {
    let crlf = texte.replace('\n', "\r\n");
    let mut octets = Vec::with_capacity(2 + crlf.len() * 2);
    octets.extend_from_slice(&[0xFF, 0xFE]);
    for unite in crlf.encode_utf16() {
        octets.extend_from_slice(&unite.to_le_bytes());
    }
    octets
}

/// La première ligne qui diffère, en clair : un `assert_eq!` sur quatre cents lignes ne
/// se lit pas.
fn premiere_difference(commite: &str, attendu: &str) -> String {
    let commites: Vec<&str> = commite.lines().collect();
    let attendues: Vec<&str> = attendu.lines().collect();
    for (index, (a, b)) in commites.iter().zip(attendues.iter()).enumerate() {
        if a != b {
            return format!("ligne {} :\n  commité : {a}\n  engendré: {b}", index + 1);
        }
    }
    format!(
        "{} lignes commitées, {} engendrées",
        commites.len(),
        attendues.len()
    )
}

// -------------------------------------------------------------------------------------
// Les tests.
// -------------------------------------------------------------------------------------

#[test]
fn inx_encode_en_utf16_le() {
    let inx = lire_inx();
    assert!(
        inx.contains("Conduit — câbles audio virtuels"),
        "le DeviceDesc accentué a été perdu au décodage"
    );
}

#[test]
fn inx_est_a_jour() {
    // Comparaison sur le texte **décodé**, fins de ligne normalisées : comparer des octets
    // bruts ferait osciller le résultat avec la configuration git de la machine.
    let commite = lire_inx().replace("\r\n", "\n");
    let attendu = inf_attendu();
    assert!(
        commite == attendu,
        "conduit-kmd/conduit_kmd.inx diffère du texte engendré par `inf_attendu` \
         ({} câbles).\n\
         Régénérez-le : cargo test -p portcls --test inf -- --ignored regenerer_inx, puis \
         commitez.\n{}",
        CABLE_COUNT,
        premiere_difference(&commite, &attendu)
    );
}

/// Réécrit `conduit_kmd.inx` (UTF-16 LE avec BOM, CRLF). `#[ignore]` : c'est une action,
/// pas une vérification.
#[test]
#[ignore = "réécrit conduit_kmd.inx ; lancer avec --ignored"]
fn regenerer_inx() {
    let chemin = chemin_inx();
    std::fs::write(&chemin, octets_utf16le(&inf_attendu()))
        .unwrap_or_else(|e| panic!("écriture de {} : {e}", chemin.display()));
    println!("{} régénéré ({CABLE_COUNT} câbles)", chemin.display());
}

/// `[ConduitCable_HW_AddReg]` : les défauts des paramètres (M1b-01) sont ceux du pilote,
/// et toute la section reste en `HKR`.
#[test]
fn defauts_de_parametres_identiques_au_pilote() {
    let inx = lire_inx();
    let section = section(&inx, "ConduitCable_HW_AddReg");

    for param in Param::ALL {
        let attendue = format!(
            "HKR,,{},0x10001,{}",
            param.value_name(),
            param.default_value()
        );
        assert!(
            section.contains(&attendue.as_str()),
            "[ConduitCable_HW_AddReg] : « {attendue} » manquante. Le pilote lit \
             « {} » dans la clé matérielle et retient {} à défaut : l'INF doit écrire \
             exactement cette valeur.\nSection commitée : {section:#?}",
            param.value_name(),
            param.default_value()
        );
    }

    // F-52 (désinstallation propre) : PnP retire les `HKR` de la clé matérielle avec le
    // périphérique. Une écriture `HKLM` survivrait à la désinstallation et laisserait le
    // registre sale — et `infverif /w` la refuserait si elle visait une clé hors du
    // périphérique.
    for ligne in &section {
        assert!(
            ligne.starts_with("HKR,"),
            "[ConduitCable_HW_AddReg] : « {ligne} » n'est pas un HKR ; PnP ne la \
             retirerait pas à la désinstallation (F-52)"
        );
    }
}

#[test]
fn noms_de_sous_peripheriques_identiques_au_pilote() {
    let inx = lire_inx();
    let strings = strings(&inx);
    for cable in 0..CABLE_COUNT {
        for ((famille, _, _), attendu) in SOUS_PERIPHERIQUES.iter().zip(noms_du_cable(cable)) {
            let jeton = format!("KSNAME_{famille}{cable}");
            let dans_inf = strings.get(&jeton).unwrap_or_else(|| {
                panic!("[Strings] : {jeton} manquant ; le pilote enregistre « {attendu} »")
            });
            assert_eq!(
                dans_inf, &attendu,
                "[Strings] {jeton} = « {dans_inf} » mais PcRegisterSubdevice reçoit \
                 « {attendu} » : le périphérique s'installerait sans publier d'endpoint"
            );
        }
    }
}

#[test]
fn add_interface_couvre_exactement_les_sous_peripheriques_de_tous_les_cables() {
    let inx = lire_inx();
    let sections = sections(&inx);
    let mut trouvees = Vec::new();
    for ligne in section(&inx, SECTION_INTERFACES) {
        let (cle, valeur) = ligne.split_once('=').expect("AddInterface sans « = »");
        assert_eq!(cle.trim(), "AddInterface", "ligne inattendue : {ligne}");
        let champs: Vec<&str> = valeur.split(',').map(str::trim).collect();
        assert_eq!(
            champs.len(),
            3,
            "AddInterface à {} champs : {ligne}",
            champs.len()
        );
        let jeton = |champ: &str| {
            champ
                .strip_prefix('%')
                .and_then(|c| c.strip_suffix('%'))
                .unwrap_or_else(|| panic!("champ non jetonisé : {champ}"))
                .to_owned()
        };
        trouvees.push((jeton(champs[0]), jeton(champs[1]), champs[2].to_owned()));
    }
    assert_eq!(
        trouvees,
        interfaces_attendues(),
        "[{SECTION_INTERFACES}] ne publie pas exactement les interfaces des \
         sous-périphériques enregistrés par conduit_kmd::adapter, câble par câble"
    );
    for (_, _, nom) in &trouvees {
        assert!(
            sections.contains(nom),
            "section d'interface absente : [{nom}]"
        );
        assert!(
            sections.contains(&format!("{nom}.AddReg")),
            "section absente : [{nom}.AddReg]"
        );
    }
}

#[test]
fn guids_de_categorie_identiques_aux_entetes_du_wdk() {
    let inx = lire_inx();
    let strings = strings(&inx);
    for (jeton, guid) in [
        ("KSCATEGORY_AUDIO", KSCATEGORY_AUDIO),
        ("KSCATEGORY_RENDER", KSCATEGORY_RENDER),
        ("KSCATEGORY_CAPTURE", KSCATEGORY_CAPTURE),
        ("KSCATEGORY_REALTIME", KSCATEGORY_REALTIME),
        ("KSCATEGORY_TOPOLOGY", KSCATEGORY_TOPOLOGY),
    ] {
        let attendu = guid_texte(&guid);
        let dans_inf = strings
            .get(jeton)
            .unwrap_or_else(|| panic!("[Strings] : {jeton} manquant"));
        assert_eq!(
            dans_inf.to_uppercase(),
            attendu,
            "[Strings] {jeton} diffère de portcls_sys::{jeton} (ksmedia.h du WDK)"
        );
    }
    assert_eq!(
        strings.get("Proxy.CLSID").map(String::as_str),
        Some(PROXY_CLSID),
        "[Strings] Proxy.CLSID doit être le CLSID du proxy KS (ks.inf)"
    );
}

#[test]
fn guids_de_nom_de_broche_identiques_au_pilote() {
    let inx = lire_inx();
    let strings = strings(&inx);
    for cable in 0..CABLE_COUNT {
        let numero = u8::try_from(cable).expect("numéro de câble sur un octet");
        let jeton = format!("GUID.PinName.Cable{cable}");
        let attendu = guid_texte(&pin_name_guid(numero));
        let dans_inf = strings
            .get(&jeton)
            .unwrap_or_else(|| panic!("[Strings] : {jeton} manquant"));
        assert_eq!(
            dans_inf.to_uppercase(),
            attendu,
            "[Strings] {jeton} diffère de portcls::pin_name_guid({cable}), le GUID \
             KsPinDescriptor.Name des broches endpoint : les endpoints garderaient le nom \
             de leur catégorie (« Haut-parleurs », « Ligne »)"
        );
        assert_eq!(
            strings
                .get(&format!("PinName.Cable{cable}"))
                .map(String::as_str),
            Some(format!("Conduit {}", cable + 1).as_str()),
            "[Strings] PinName.Cable{cable} est le nom que Windows affiche"
        );
    }
    // Seize câbles, seize GUID **distincts** : deux endpoints homonymes seraient
    // indistinguables dans le panneau de son.
    let distincts: BTreeSet<&String> = (0..CABLE_COUNT)
        .filter_map(|cable| strings.get(&format!("GUID.PinName.Cable{cable}")))
        .collect();
    assert_eq!(
        distincts.len(),
        CABLE_COUNT,
        "les GUID de nom de broche doivent être deux à deux distincts"
    );
    assert_eq!(
        strings.get("MediaCategories").map(String::as_str),
        Some("MediaCategories"),
        "le nom de broche s'enregistre sous HKR\\MediaCategories (clé logicielle du \
         périphérique), pas dans l'espace global HKLM"
    );
    assert_eq!(
        section(&inx, "ConduitCable_PinNames_AddReg"),
        media_categories_attendues(),
        "[ConduitCable_PinNames_AddReg] doit associer le GUID de nom de broche de chaque \
         câble à sa chaîne"
    );
}

#[test]
fn tous_les_jetons_sont_definis_et_tous_les_noms_servent() {
    let inx = lire_inx();
    let strings = strings(&inx);
    let utilises = jetons_utilises(&inx);
    for jeton in &utilises {
        assert!(
            strings.contains_key(jeton),
            "%{jeton}% est utilisé mais absent de [Strings]"
        );
    }
    // Et la réciproque : à quatre cents lignes engendrées, c'est elle qui attrape une
    // coquille de numéro (un `KSNAME_WaveRender7` défini, jamais publié).
    for jeton in strings.keys() {
        assert!(
            utilises.contains(jeton),
            "%{jeton}% est défini dans [Strings] mais jamais utilisé"
        );
    }
}
