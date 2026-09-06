//! Cohérence entre `conduit-kmd/conduit_kmd.inx` et les constantes Rust (M1a-09,
//! driver-design.md §4.1 et §4.2).
//!
//! L'INF et le pilote se donnent rendez-vous sur des chaînes et des GUID qu'aucun
//! compilateur ne rapproche :
//!
//! | Dans l'INF | Dans le pilote | Symptôme d'une divergence |
//! |---|---|---|
//! | nom de référence d'un `AddInterface` | `WAVE_RENDER_0`… passé à `PcRegisterSubdevice` | le périphérique s'installe, **aucun endpoint** n'apparaît |
//! | `GUID.PinName.Cable0` | `pin_name_guid(0)` = `KsPinDescriptor.Name` | l'endpoint s'appelle « Haut-parleurs » / « Ligne » |
//! | `%KSCATEGORY_*%` | `portcls_sys::KSCATEGORY_*` (en-têtes du WDK) | interface publiée dans la mauvaise classe |
//!
//! Ces pannes ne se voient que dans une VM, et mal. Ce test les transforme en échec de
//! `cargo test -p portcls`, donc de `tools/check.ps1` et de la CI.
//!
//! Il vérifie aussi l'**encodage** de la copie de travail : un INF non ASCII doit être en
//! UTF-16 LE (« General Guidelines for INF Files »), ce que `.gitattributes` obtient par
//! `working-tree-encoding=UTF-16LE-BOM` ; sans le BOM, `stampinf` recopierait de l'UTF-8
//! que SetupAPI relirait en page de codes ANSI (« câbles » → « câbles »).
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

use portcls::{TOPO_CAPTURE_0, TOPO_RENDER_0, WAVE_CAPTURE_0, WAVE_RENDER_0, pin_name_guid};
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

/// Les `AddInterface` attendus : (jeton de catégorie, jeton de nom, section). Le tableau
/// reflète ce que PortCls enregistre — `CLSID_PortWaveRT` pour les filtres wave (audio,
/// rendu ou capture, temps réel), `CLSID_PortTopology` pour les filtres topologie (audio,
/// topologie) — et l'INF doit le publier exactement.
const INTERFACES_ATTENDUES: &[(&str, &str, &str)] = &[
    (
        "KSCATEGORY_AUDIO",
        "KSNAME_WaveRender0",
        "WaveRender0.Interface",
    ),
    (
        "KSCATEGORY_RENDER",
        "KSNAME_WaveRender0",
        "WaveRender0.Interface",
    ),
    (
        "KSCATEGORY_REALTIME",
        "KSNAME_WaveRender0",
        "WaveRender0.Interface",
    ),
    (
        "KSCATEGORY_AUDIO",
        "KSNAME_TopoRender0",
        "TopoRender0.Interface",
    ),
    (
        "KSCATEGORY_TOPOLOGY",
        "KSNAME_TopoRender0",
        "TopoRender0.Interface",
    ),
    (
        "KSCATEGORY_AUDIO",
        "KSNAME_WaveCapture0",
        "WaveCapture0.Interface",
    ),
    (
        "KSCATEGORY_CAPTURE",
        "KSNAME_WaveCapture0",
        "WaveCapture0.Interface",
    ),
    (
        "KSCATEGORY_REALTIME",
        "KSNAME_WaveCapture0",
        "WaveCapture0.Interface",
    ),
    (
        "KSCATEGORY_AUDIO",
        "KSNAME_TopoCapture0",
        "TopoCapture0.Interface",
    ),
    (
        "KSCATEGORY_TOPOLOGY",
        "KSNAME_TopoCapture0",
        "TopoCapture0.Interface",
    ),
];

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

/// Une constante de nom de sous-périphérique, en texte.
fn nom_utf16(nom: &[u16]) -> String {
    let (nul, texte) = nom.split_last().expect("nom vide");
    assert_eq!(*nul, 0, "nom de sous-périphérique non terminé par NUL");
    String::from_utf16(texte).expect("nom de sous-périphérique non UTF-16")
}

#[test]
fn inx_encode_en_utf16_le() {
    let inx = lire_inx();
    assert!(
        inx.contains("Conduit — câbles audio virtuels"),
        "le DeviceDesc accentué a été perdu au décodage"
    );
}

#[test]
fn noms_de_sous_peripheriques_identiques_au_pilote() {
    let inx = lire_inx();
    let strings = strings(&inx);
    for (jeton, constante) in [
        ("KSNAME_WaveRender0", &WAVE_RENDER_0[..]),
        ("KSNAME_TopoRender0", &TOPO_RENDER_0[..]),
        ("KSNAME_WaveCapture0", &WAVE_CAPTURE_0[..]),
        ("KSNAME_TopoCapture0", &TOPO_CAPTURE_0[..]),
    ] {
        let attendu = nom_utf16(constante);
        let dans_inf = strings.get(jeton).unwrap_or_else(|| {
            panic!("[Strings] : {jeton} manquant ; le pilote enregistre « {attendu} »")
        });
        assert_eq!(
            dans_inf, &attendu,
            "[Strings] {jeton} = « {dans_inf} » mais PcRegisterSubdevice reçoit \
             « {attendu} » : le périphérique s'installerait sans publier d'endpoint"
        );
    }
}

#[test]
fn add_interface_couvre_exactement_les_quatre_sous_peripheriques() {
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
    let attendues: Vec<(String, String, String)> = INTERFACES_ATTENDUES
        .iter()
        .map(|(c, n, s)| ((*c).to_owned(), (*n).to_owned(), (*s).to_owned()))
        .collect();
    assert_eq!(
        trouvees, attendues,
        "[{SECTION_INTERFACES}] ne publie pas exactement les interfaces des quatre \
         sous-périphériques enregistrés par conduit_kmd::adapter"
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
fn guid_de_nom_de_broche_identique_au_pilote() {
    let inx = lire_inx();
    let strings = strings(&inx);
    let attendu = guid_texte(&pin_name_guid(0));
    let dans_inf = strings
        .get("GUID.PinName.Cable0")
        .unwrap_or_else(|| panic!("[Strings] : GUID.PinName.Cable0 manquant"));
    assert_eq!(
        dans_inf.to_uppercase(),
        attendu,
        "[Strings] GUID.PinName.Cable0 diffère de portcls::pin_name_guid(0), le GUID \
         KsPinDescriptor.Name des broches endpoint : les endpoints garderaient le nom de \
         leur catégorie (« Haut-parleurs », « Ligne »)"
    );
    assert_eq!(
        strings.get("PinName.Cable0").map(String::as_str),
        Some("Conduit 1"),
        "[Strings] PinName.Cable0 est le nom que Windows affiche"
    );
    assert_eq!(
        strings.get("MediaCategories").map(String::as_str),
        Some("MediaCategories"),
        "le nom de broche s'enregistre sous HKR\\MediaCategories (clé logicielle du \
         périphérique), pas dans l'espace global HKLM"
    );
    let attendu_addreg =
        "HKR,%MediaCategories%\\%GUID.PinName.Cable0%,Name,,%PinName.Cable0%".to_owned();
    assert_eq!(
        section(&inx, "ConduitCable_PinNames_AddReg"),
        vec![attendu_addreg.as_str()],
        "[ConduitCable_PinNames_AddReg] doit associer le GUID de nom de broche à la chaîne"
    );
}

#[test]
fn tous_les_jetons_sont_definis_et_tous_les_noms_servent() {
    let inx = lire_inx();
    let strings = strings(&inx);
    for jeton in jetons_utilises(&inx) {
        assert!(
            strings.contains_key(&jeton),
            "%{jeton}% est utilisé mais absent de [Strings]"
        );
    }
    for jeton in ["KSNAME_WaveRender0", "KSNAME_TopoRender0"] {
        assert!(
            jetons_utilises(&inx).contains(jeton),
            "%{jeton}% est défini mais jamais utilisé"
        );
    }
}
