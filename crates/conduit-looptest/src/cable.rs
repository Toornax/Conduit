//! Les actions `--cable-*`, sur Windows : parler au jeu de propriétés KS privé du
//! pilote (`KSPROPSETID_Conduit`, M1b-04).
//!
//! Rien n'est réécrit du transport : tout passe par `conduit_backend_wasapi::cable`
//! ([`TopologyFilter`], [`BadInput`], [`OsError`]), qui lui-même tient son contrat de
//! `conduit-kmd-core`, partagé avec le pilote. Ce module ne fait que **choisir** les
//! requêtes et mettre en forme le compte rendu.
//!
//! Quatre actions, combinables dans un seul appel et exécutées dans cet ordre :
//!
//! 1. `--cable-etat` : l'état des seize câbles, plus la version du contrat servi ;
//! 2. `--cable-set` : l'écriture, affichée avant et après ;
//! 3. `--cable-chrono` : le délai entre l'écriture et l'endpoint MMDevice qui suit ;
//! 4. `--cable-invalide` : la batterie d'entrées volontairement invalides.
//!
//! **Aucun flux audio n'est ouvert et aucun son n'émis** : `IOCTL_KS_PROPERTY` est une
//! requête de contrôle, et le chronomètre ne fait qu'**énumérer** les endpoints. Les
//! seules interfaces atteintes sont celles du pilote Conduit, qui n'existent pas sur
//! une machine sans lui.

#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::thread::sleep;
use std::time::{Duration, Instant};

use conduit_backend::{Backend, CableId, DeviceDirection};
use conduit_backend_wasapi::cable::{
    contract_version, topology_interfaces, BadInput, CableConfigError, CableState, FilterSide,
    TopologyFilter, CABLE_MAX,
};
use conduit_backend_wasapi::WasapiBackend;

use crate::cli::{Args, CoteCable, EtatCable};

/// Pas du sondage du chronomètre.
///
/// Cinq millisecondes : assez fin pour que la mesure ait un sens face au seuil d'une
/// seconde du critère F-01, assez large pour qu'une énumération MMDevice — qui n'est
/// pas gratuite — ait le temps de rendre la main entre deux tours.
const PAS_CHRONO: Duration = Duration::from_millis(5);

/// Ce qu'une action `--cable-*` a produit.
#[derive(Debug)]
pub struct Rapport {
    /// Le compte rendu à imprimer.
    pub texte: String,
    /// Faux si quelque chose de contraire au critère a été observé : une entrée
    /// invalide **acceptée**, un endpoint qui n'a pas suivi dans le délai imparti.
    pub conforme: bool,
}

impl From<CoteCable> for FilterSide {
    fn from(cote: CoteCable) -> Self {
        match cote {
            CoteCable::Rendu => Self::Render,
            CoteCable::Capture => Self::Capture,
        }
    }
}

/// Exécute les actions `--cable-*` demandées et rend leur compte rendu.
///
/// # Erreurs
///
/// Message en français quand l'environnement ne permet pas l'action : pilote absent,
/// énumération refusée, filtre impossible à ouvrir. Un **refus du pilote** sur une
/// requête, lui, n'est pas une erreur d'environnement : il est affiché avec son code,
/// puisque c'est précisément ce qu'on cherche à lire.
pub fn run(args: &Args) -> Result<Rapport, String> {
    let side = FilterSide::from(args.cable_cote);
    let paths = topology_interfaces().map_err(|e| e.to_string())?;
    let mut texte = String::new();
    let mut conforme = true;

    if args.cable_etat {
        texte.push_str(&etat_des_cables(&paths, side));
    }

    if let Some(vise) = args.cable_set {
        let cable = cible(args)?;
        let (bloc, ok) = ecrire(&paths, cable, side, vise, args)?;
        pousser(&mut texte, &bloc);
        conforme &= ok;
    }

    if args.cable_invalide {
        let cable = cible(args)?;
        let (bloc, ok) = entrees_invalides(&paths, cable, side)?;
        pousser(&mut texte, &bloc);
        conforme &= ok;
    }

    Ok(Rapport { texte, conforme })
}

/// Ajoute un bloc au compte rendu, séparé du précédent par une ligne vide.
fn pousser(texte: &mut String, bloc: &str) {
    if !texte.is_empty() {
        texte.push('\n');
    }
    texte.push_str(bloc);
}

/// Le câble visé par une action qui écrit ; [`Args::validate`] a déjà refusé son
/// absence, ce chemin ne sert donc que de garde.
fn cible(args: &Args) -> Result<CableId, String> {
    args.cable_vise()
        .map(CableId)
        .ok_or_else(|| "--cable-set et --cable-invalide demandent --cable N (1 à 16)".to_string())
}

/// Ouvre le filtre d'un câble ; l'absence de filtre n'est pas une erreur fatale ici.
fn ouvrir(
    paths: &[String],
    cable: CableId,
    side: FilterSide,
) -> Result<TopologyFilter, CableConfigError> {
    TopologyFilter::open_in(paths, cable, side)
}

/// L'état d'un câble, en une ligne.
fn ligne_etat(cable: CableId, side: FilterSide, etat: &CableState, index: u32) -> String {
    format!(
        "  Conduit {:>2} (index pilote {index}, {}{index}) : {}, {} canaux\n",
        cable.0,
        side.prefix(),
        if etat.is_connected() {
            "connecté  "
        } else {
            "déconnecté"
        },
        etat.channels
    )
}

/// `--cable-etat` : l'état des seize câbles et la version du contrat servi.
fn etat_des_cables(paths: &[String], side: FilterSide) -> String {
    let mut out = format!(
        "état des câbles, côté {} (filtres {}<n>) :\n",
        side.label(),
        side.prefix()
    );
    let mut version: Option<Result<u32, String>> = None;
    let mut vus = 0u32;
    for numero in 1..=CABLE_MAX {
        let cable = CableId(numero);
        // Décalage de un : « Conduit 1 » est l'index 0 du pilote, celui qui suffixe
        // `TopoRender<n>` et celui qui voyage dans `CableState::cable`.
        let index = numero.saturating_sub(1);
        match ouvrir(paths, cable, side) {
            Ok(filtre) => {
                vus = vus.saturating_add(1);
                if version.is_none() {
                    version = Some(filtre.read_version().map_err(|e| e.to_string()));
                }
                match filtre.read_state() {
                    Ok(etat) => out.push_str(&ligne_etat(cable, side, &etat, index)),
                    Err(e) => {
                        let _ = writeln!(
                            out,
                            "  Conduit {:>2} (index pilote {index}) : lecture refusée — {e}",
                            cable.0
                        );
                    }
                }
            }
            Err(CableConfigError::FiltreAbsent { .. }) => {
                let _ = writeln!(
                    out,
                    "  Conduit {:>2} (index pilote {index}) : filtre absent",
                    cable.0
                );
            }
            Err(e) => {
                let _ = writeln!(out, "  Conduit {:>2} : {e}", cable.0);
            }
        }
    }
    let _ = writeln!(out, "{vus} filtre(s) de topologie Conduit sur {CABLE_MAX}.");
    match version {
        Some(Ok(servie)) => {
            let attendue = contract_version();
            let _ = writeln!(
                out,
                "KSPROPERTY_CONDUIT_VERSION : {servie} (contrat de cet outil : {attendue}){}",
                if servie == attendue {
                    ""
                } else {
                    " — INADÉQUATION : la structure d'échange n'a pas la forme attendue, \
                     ne vous fiez pas aux états ci-dessus"
                }
            );
        }
        Some(Err(e)) => {
            let _ = writeln!(out, "KSPROPERTY_CONDUIT_VERSION : lecture refusée — {e}");
        }
        None => out.push_str(
            "Aucun filtre de topologie Conduit : le pilote n'est pas chargé sur cette \
             machine (voir docs/driver-dev.md).\n",
        ),
    }
    out
}

/// `--cable-set` (et `--cable-chrono`) : écrit l'état, l'affiche avant et après, et
/// chronomètre éventuellement les endpoints.
fn ecrire(
    paths: &[String],
    cable: CableId,
    side: FilterSide,
    vise: EtatCable,
    args: &Args,
) -> Result<(String, bool), String> {
    let filtre = ouvrir(paths, cable, side).map_err(|e| e.to_string())?;
    let index = filtre.index();
    let avant = filtre.read_state().map_err(|e| e.to_string())?;
    let mut out = format!(
        "écriture sur Conduit {} (index pilote {index}, {}{index})\n",
        cable.0,
        side.prefix()
    );
    let _ = writeln!(
        out,
        "    avant  : {}, {} canaux",
        if avant.is_connected() {
            "connecté"
        } else {
            "déconnecté"
        },
        avant.channels
    );
    let _ = writeln!(out, "    demandé : {}", vise.label());

    // Le backend est ouvert **avant** l'écriture : sa création coûte un fil et un
    // appartement COM, qui n'ont rien à faire dans le délai mesuré.
    let backend = if args.cable_chrono {
        Some(WasapiBackend::new().map_err(|e| format!("backend WASAPI indisponible : {e}"))?)
    } else {
        None
    };
    let depart_deja_bon = match backend.as_ref() {
        Some(backend) => Some(presence(backend, cable)?),
        None => None,
    };

    if let Err(e) = filtre.write_state(vise.connecte()) {
        let _ = writeln!(out, "    écriture REFUSÉE : {e}");
        let note = note_privilege(&e);
        if !note.is_empty() {
            let _ = writeln!(out, "{note}");
        }
        return Ok((out, false));
    }
    let ecrit_a = Instant::now();
    let apres = filtre.read_state().map_err(|e| e.to_string())?;
    let _ = writeln!(
        out,
        "    après  : {}, {} canaux",
        if apres.is_connected() {
            "connecté"
        } else {
            "déconnecté"
        },
        apres.channels
    );
    let mut conforme = apres.is_connected() == vise.connecte();
    if !conforme {
        out.push_str(
            "    ANOMALIE : la propriété a été acceptée mais la relecture ne montre pas \
             l'état demandé\n",
        );
    }

    if let (Some(backend), Some(depart)) = (backend.as_ref(), depart_deja_bon) {
        let (bloc, ok) = chronometrer(
            backend,
            cable,
            vise,
            depart,
            ecrit_a,
            Duration::from_millis(args.cable_chrono_max_ms),
        );
        out.push_str(&bloc);
        conforme &= ok;
    }
    Ok((out, conforme))
}

/// Les deux endpoints du câble, présents ou non, tels que MMDevice les voit à
/// l'instant de l'appel.
///
/// « Présent » veut dire **actif** : un câble déconnecté présente ses prises comme
/// vides, et Windows range alors ses endpoints sous « Périphériques déconnectés », hors
/// de `DEVICE_STATE_ACTIVE`. C'est exactement l'observation du critère F-01.
fn presence(backend: &WasapiBackend, cable: CableId) -> Result<[bool; 2], String> {
    let devices = backend
        .devices()
        .map_err(|e| format!("énumération des endpoints : {e}"))?;
    let vu = |direction: DeviceDirection| {
        devices
            .iter()
            .any(|d| d.cable == Some(cable) && d.direction == direction)
    };
    Ok([vu(DeviceDirection::Render), vu(DeviceDirection::Capture)])
}

/// Attend que les deux endpoints du câble aient suivi l'état écrit, et rend le délai.
///
/// `ecrit_a` est l'instant où `DeviceIoControl` a rendu la main : c'est là que commence
/// le délai du critère F-01, pas à l'ouverture du backend — celle-ci a lieu avant
/// l'écriture, précisément pour ne pas entrer dans la mesure.
///
/// Le délai rendu est une **borne supérieure** : il est relevé après l'énumération qui
/// a vu le changement, laquelle n'est pas gratuite. Un « moins d'une seconde » affiché
/// ici en est donc réellement un ; un délai qui frôle la seconde mérite un second
/// relevé avant d'accuser le pilote.
fn chronometrer(
    backend: &WasapiBackend,
    cable: CableId,
    vise: EtatCable,
    depart: [bool; 2],
    ecrit_a: Instant,
    max: Duration,
) -> (String, bool) {
    let attendu = vise.connecte();
    let mut delais: [Option<Duration>; 2] = [None, None];
    let mut out = format!(
        "    chronomètre : endpoints « Conduit {} » attendus {}\n",
        cable.0,
        if attendu { "présents" } else { "absents" }
    );
    loop {
        match presence(backend, cable) {
            Ok(etat) => {
                let maintenant = ecrit_a.elapsed();
                for (index, present) in etat.iter().enumerate() {
                    if *present == attendu && delais.get(index).is_some_and(Option::is_none) {
                        if let Some(place) = delais.get_mut(index) {
                            *place = Some(maintenant);
                        }
                    }
                }
            }
            Err(e) => {
                let _ = writeln!(out, "      énumération interrompue : {e}");
                break;
            }
        }
        if delais.iter().all(Option::is_some) || ecrit_a.elapsed() >= max {
            break;
        }
        sleep(PAS_CHRONO);
    }

    let mut conforme = true;
    for (index, (sens, etait)) in [("rendu", depart[0]), ("capture", depart[1])]
        .iter()
        .enumerate()
    {
        let deja = *etait == attendu;
        match delais.get(index).copied().flatten() {
            Some(_) if deja => {
                let _ = writeln!(
                    out,
                    "      {sens}   : déjà {} avant l'écriture, rien à mesurer",
                    if attendu { "présent" } else { "absent" }
                );
            }
            Some(delai) => {
                let ms = delai.as_secs_f64() * 1000.0;
                let _ = writeln!(
                    out,
                    "      {sens}   : {} après {ms:.0} ms{}",
                    if attendu { "apparu" } else { "disparu" },
                    if delai <= Duration::from_secs(1) {
                        " (F-01 : sous la seconde)"
                    } else {
                        " — AU-DELÀ DE LA SECONDE du critère F-01"
                    }
                );
                conforme &= delai <= Duration::from_secs(1);
            }
            None => {
                let _ = writeln!(
                    out,
                    "      {sens}   : toujours {} après {} ms d'attente — le changement de \
                     prise n'a pas atteint MMDevice",
                    if attendu { "absent" } else { "présent" },
                    max.as_millis()
                );
                conforme = false;
            }
        }
    }
    (out, conforme)
}

/// `--cable-invalide` : la batterie d'entrées volontairement invalides.
///
/// Chaque entrée **doit** échouer ; le code Win32 est affiché tel quel, c'est lui qui
/// dit quel contrôle a joué. L'état du câble est relu avant et après : une entrée
/// invalide ne doit avoir **aucun effet**, ce qui est l'autre moitié du critère.
fn entrees_invalides(
    paths: &[String],
    cable: CableId,
    side: FilterSide,
) -> Result<(String, bool), String> {
    let filtre = ouvrir(paths, cable, side).map_err(|e| e.to_string())?;
    let index = filtre.index();
    let avant = filtre.read_state().map_err(|e| e.to_string())?;
    let mut out = format!(
        "entrées invalides sur Conduit {} (index pilote {index}, {}{index})\n",
        cable.0,
        side.prefix()
    );
    let _ = writeln!(
        out,
        "    état avant : {}, {} canaux",
        if avant.is_connected() {
            "connecté"
        } else {
            "déconnecté"
        },
        avant.channels
    );

    let mut refusees = 0u32;
    let mut privileges = 0u32;
    for mauvaise in BadInput::ALL {
        let charge = mauvaise.payload(index);
        let resultat = filtre.write_raw(&charge);
        match resultat {
            Ok(traites) => {
                let _ = writeln!(
                    out,
                    "    {:<34} ({:>2} octets) → ACCEPTÉE ({traites} octets traités) : ANOMALIE, \
                     le pilote aurait dû refuser",
                    mauvaise.label(),
                    charge.len()
                );
            }
            Err(CableConfigError::Requete { erreur, .. }) => {
                refusees = refusees.saturating_add(1);
                if erreur.win32() == Some(1314) {
                    privileges = privileges.saturating_add(1);
                }
                let _ = writeln!(
                    out,
                    "    {:<34} ({:>2} octets) → refusée : {erreur}",
                    mauvaise.label(),
                    charge.len()
                );
            }
            Err(autre) => {
                refusees = refusees.saturating_add(1);
                let _ = writeln!(
                    out,
                    "    {:<34} ({:>2} octets) → refusée : {autre}",
                    mauvaise.label(),
                    charge.len()
                );
            }
        }
    }

    let total = u32::try_from(BadInput::ALL.len()).unwrap_or(u32::MAX);
    let apres = filtre.read_state().map_err(|e| e.to_string())?;
    let inchange = apres == avant;
    let _ = writeln!(
        out,
        "    état après : {}, {} canaux{}",
        if apres.is_connected() {
            "connecté"
        } else {
            "déconnecté"
        },
        apres.channels,
        if inchange {
            " (inchangé, comme attendu)"
        } else {
            " — ANOMALIE : une entrée invalide a eu un effet"
        }
    );
    let _ = writeln!(out, "    verdict : {refusees} refus sur {total}");
    if privileges == total {
        out.push_str(
            "    Note : les six refus sont des ERROR_PRIVILEGE_NOT_HELD. Le pilote contrôle \
             le privilège AVANT de valider le contenu, par conception — relancez depuis un \
             processus élevé pour éprouver la validation elle-même.\n",
        );
    }
    Ok((out, refusees == total && inchange))
}

/// Le mot à ajouter quand un refus d'écriture ressemble à un manque de privilège.
fn note_privilege(erreur: &CableConfigError) -> &'static str {
    match erreur {
        CableConfigError::Requete { erreur, .. } if erreur.win32() == Some(1314) => {
            "    L'écriture exige SE_LOAD_DRIVER_PRIVILEGE : relancez depuis un processus \
             élevé (le service d'assistance de M1b-20 tournera en LocalSystem)."
        }
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use conduit_backend_wasapi::cable::OsError;

    use super::*;

    #[test]
    fn le_cote_de_la_ligne_de_commande_se_traduit() {
        assert_eq!(FilterSide::from(CoteCable::Rendu), FilterSide::Render);
        assert_eq!(FilterSide::from(CoteCable::Capture), FilterSide::Capture);
    }

    #[test]
    fn les_blocs_sont_separes_par_une_ligne_vide() {
        let mut texte = String::new();
        pousser(&mut texte, "un\n");
        assert_eq!(texte, "un\n");
        pousser(&mut texte, "deux\n");
        assert_eq!(texte, "un\n\ndeux\n");
    }

    #[test]
    fn la_ligne_d_etat_dit_le_cable_son_index_et_son_filtre() {
        let etat = CableState::new(0, true);
        let ligne = ligne_etat(CableId(1), FilterSide::Render, &etat, 0);
        // Le décalage de un, lisible dans la sortie : Conduit 1 → index 0 → TopoRender0.
        assert!(ligne.contains("Conduit  1"), "{ligne}");
        assert!(ligne.contains("index pilote 0"), "{ligne}");
        assert!(ligne.contains("TopoRender0"), "{ligne}");
        assert!(ligne.contains("connecté"), "{ligne}");

        let etat = CableState::new(15, false);
        let ligne = ligne_etat(CableId(16), FilterSide::Capture, &etat, 15);
        assert!(ligne.contains("Conduit 16"), "{ligne}");
        assert!(ligne.contains("TopoCapture15"), "{ligne}");
        assert!(ligne.contains("déconnecté"), "{ligne}");
    }

    #[test]
    fn la_note_de_privilege_ne_sort_que_sur_1314() {
        let refus = CableConfigError::Requete {
            propriete: "KSPROPERTY_CONDUIT_CABLE_STATE",
            verbe: "SET",
            erreur: OsError::Win32(1314),
        };
        assert!(note_privilege(&refus).contains("SE_LOAD_DRIVER_PRIVILEGE"));
        let autre = CableConfigError::Requete {
            propriete: "KSPROPERTY_CONDUIT_CABLE_STATE",
            verbe: "SET",
            erreur: OsError::Win32(87),
        };
        assert_eq!(note_privilege(&autre), "");
    }
}
