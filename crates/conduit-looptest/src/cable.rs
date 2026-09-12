//! Les actions `--cable-*`, sur Windows : parler au jeu de propriétés KS privé du
//! pilote (`KSPROPSETID_Conduit`, M1b-04).
//!
//! Rien n'est réécrit du transport : tout passe par `conduit_backend_wasapi::cable`
//! ([`TopologyFilter`], [`BadInput`], [`OsError`]), qui lui-même tient son contrat de
//! `conduit-kmd-core`, partagé avec le pilote. Ce module ne fait que **choisir** les
//! requêtes et mettre en forme le compte rendu.
//!
//! Sept actions, combinables dans un seul appel et exécutées dans cet ordre :
//!
//! 1. `--cable-privilege` : l'état de `SeLoadDriverPrivilege` dans ce processus, sans
//!    rien écrire ni armer ;
//! 2. `--cable-etat` : l'état des seize câbles, plus la version du contrat servi ;
//! 3. `--cable-compteurs` : les compteurs de la boucle locale, et le régime qu'ils
//!    démontrent — la lecture de M1b-07 que seul le débogueur savait faire (M1b-21) ;
//! 4. `--cable-transport` : par sens, si le pilote a **déclaré ses contraintes de taille de
//!    paquet** au démarrage (`DEVPKEY_KsAudio_PacketSize_Constraints2` — sans elles, aucune
//!    période plus courte que 10 ms n'est demandable), comment le moteur audio a alloué le
//!    tampon (scrutation ou notifications) et combien d'allocations nous avons refusées — la
//!    question du lot 0 du mode paquets WaveRT —, **puis** ce qu'il a fait des interfaces
//!    du mode paquets : le `PacketMode` effectif, ce que chaque flux expose, les
//!    `QueryInterface` reçus et rendus, et les appels **servis** par méthode, avec leur
//!    IRQL et leurs horodatages (lot 2 pour le relevé, lot 3 pour ce qu'il compte). Les
//!    deux relevés répondent à la même question et sortent ensemble ;
//! 5. `--cable-set` : l'écriture, affichée avant et après ;
//! 6. `--cable-chrono` : le délai entre l'écriture et l'endpoint MMDevice qui suit ;
//! 7. `--cable-invalide` : la batterie d'entrées volontairement invalides.
//!
//! # Le privilège est armé une fois, pour toute la durée des écritures
//!
//! Le pilote exige `SeLoadDriverPrivilege` **actif**, et Windows livre les jetons avec
//! leurs privilèges désactivés — élévation et `LocalSystem` compris, ce que la mesure
//! en machine virtuelle a établi. [`run`] appelle donc [`armer_privilege`] **une seule
//! fois**, avant les actions qui écrivent, et garde le garde jusqu'à son retour : il
//! restaure alors le jeton dans l'état où il l'a trouvé. Une lecture
//! (`--cable-privilege`, `--cable-etat`, `--cable-compteurs`, `--cable-transport`) n'arme
//! rien, parce qu'elle n'en a pas besoin.
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
    armer_privilege, contract_version, etat_privilege, topology_interfaces, Armement, BadInput,
    CableConfigError, CableCounters, CablePackets, CableState, CableTransport, EtatPrivilege,
    FilterSide, StreamPackets, StreamSide, StreamTransport, TopologyFilter, CABLE_MAX,
    CONSTRAINTS_NON_TENTEE,
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
    let mut texte = String::new();
    let mut conforme = true;

    // L'état du privilège se lit sans le pilote, et même sans câble : il passe donc
    // **avant** l'énumération, pour que `--cable-privilege` réponde sur n'importe quelle
    // machine — y compris celle où le diagnostic est le plus utile, celle où rien ne
    // marche.
    if args.cable_privilege {
        pousser(&mut texte, &bloc_privilege(&etat_privilege()));
    }
    if !(args.cable_etat || args.cable_compteurs || args.cable_transport || args.writes_cable()) {
        return Ok(Rapport { texte, conforme });
    }

    let side = FilterSide::from(args.cable_cote);
    let paths = topology_interfaces().map_err(|e| e.to_string())?;

    if args.cable_etat {
        pousser(&mut texte, &etat_des_cables(&paths, side));
    }

    if args.cable_compteurs {
        pousser(&mut texte, &compteurs_des_cables(&paths, side, args.cable));
    }

    if args.cable_transport {
        pousser(&mut texte, &transport_des_cables(&paths, side, args.cable));
    }

    // Une seule fois, pour toutes les écritures : le garde vit jusqu'au `return` de
    // cette fonction et restaure alors le jeton. `constate` est l'état **après**
    // tentative d'armement — c'est lui, et non un conseil d'élévation, qui explique un
    // refus persistant.
    let armement = if args.writes_cable() {
        let (issue, garde) = armer_privilege().map_err(|e| e.to_string())?;
        pousser(&mut texte, &bloc_armement(issue));
        Some((issue, garde))
    } else {
        None
    };
    let constate = armement.as_ref().map(|(issue, _)| etat_apres(*issue));

    if let Some(vise) = args.cable_set {
        let cable = cible(args)?;
        let (bloc, ok) = ecrire(&paths, cable, side, vise, args, constate)?;
        pousser(&mut texte, &bloc);
        conforme &= ok;
    }

    if args.cable_invalide {
        let cable = cible(args)?;
        let (bloc, ok) = entrees_invalides(&paths, cable, side, constate)?;
        pousser(&mut texte, &bloc);
        conforme &= ok;
    }

    Ok(Rapport { texte, conforme })
}

/// L'état du privilège que l'issue d'un armement **établit**, sans relire le jeton.
///
/// Fonction pure : [`Armement`] porte déjà les trois cas, et les relire coûterait un
/// appel de plus pour la même réponse.
fn etat_apres(issue: Armement) -> EtatPrivilege {
    match issue {
        Armement::Arme => EtatPrivilege::Actif,
        Armement::Absent => EtatPrivilege::Absent,
        Armement::NonActivable { .. } => EtatPrivilege::Desactive,
    }
}

/// `--cable-privilege` : l'état du privilège d'écriture, sans rien modifier.
fn bloc_privilege(etat: &Result<EtatPrivilege, CableConfigError>) -> String {
    match etat {
        Ok(etat) => format!(
            "privilège d'écriture SeLoadDriverPrivilege dans ce processus : {etat}\n{}\n",
            explication_privilege(*etat)
        ),
        Err(e) => format!(
            "privilège d'écriture SeLoadDriverPrivilege : état INCONNU — {e}\n    Sans cet \
             état, un refus 1314 ne peut pas être attribué : ni au compte, ni au pilote.\n"
        ),
    }
}

/// Ce qu'un état de privilège veut dire, en français.
///
/// Pure, et c'est le seul endroit où le raisonnement est écrit : la mesure a montré que
/// l'élévation ne suffit pas, il ne faut donc plus jamais conseiller « relancez élevé ».
fn explication_privilege(etat: EtatPrivilege) -> &'static str {
    match etat {
        EtatPrivilege::Absent => {
            "    Ce compte ne le détient pas : ni l'élévation ni un armement ne l'y ajouteront. \
             Il faut un compte administrateur. Toute écriture sera refusée en 1314, et ce refus \
             ne dira rien du pilote."
        }
        EtatPrivilege::Desactive => {
            "    C'est l'état NORMAL au démarrage d'un processus, y compris élevé et y compris \
             LocalSystem : Windows n'active pas un privilège tant que le processus ne l'a pas \
             armé, et le pilote l'exige ACTIF (SeSinglePrivilegeCheck). Cette option, elle, \
             n'arme rien : c'est --cable-set et --cable-invalide qui arment, le temps de leurs \
             écritures."
        }
        EtatPrivilege::Actif => {
            "    Armé : une écriture refusée en ERROR_PRIVILEGE_NOT_HELD ne s'explique plus par \
             le compte."
        }
    }
}

/// Le bloc d'armement, en tête des actions qui écrivent.
fn bloc_armement(issue: Armement) -> String {
    let mut out = format!("armement du privilège d'écriture : {issue}\n");
    let conseil = conseil_armement(issue);
    if !conseil.is_empty() {
        let _ = writeln!(out, "{conseil}");
    }
    out
}

/// Ce qu'il faut faire d'une issue d'armement ; vide quand il n'y a rien à dire.
fn conseil_armement(issue: Armement) -> &'static str {
    match issue {
        Armement::Arme => "",
        Armement::Absent => {
            "    Les écritures qui suivent vont être refusées en ERROR_PRIVILEGE_NOT_HELD, et ce \
             refus ne dira rien du pilote. Relancez depuis un compte administrateur."
        }
        Armement::NonActivable { .. } => {
            "    Cas rare : le privilège est bien dans le jeton et l'activation n'a pas pris. \
             Signalez-le tel quel ; les écritures qui suivent vont être refusées."
        }
    }
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
                    // Ce que cet outil sait, et rien de plus : les deux numéros diffèrent.
                    // Il ne sait **pas** ce qui diffère — un changement peut être purement
                    // additif (une propriété qui apparaît, M1b-21) et laisser l'état
                    // parfaitement lisible, comme il peut toucher à la structure
                    // d'échange. Affirmer la seconde hypothèse serait un diagnostic faux
                    // une fois sur deux.
                    " — INADÉQUATION : le pilote ne sert pas le millésime de contrat que \
                     cet outil connaît. Ce qui diffère n'est pas dit ici : reprenez la \
                     documentation de CONFIG_VERSION avant de vous fier aux états \
                     ci-dessus"
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

/// Le régime que six compteurs démontrent, en français et en une ligne.
///
/// Pure, et c'est ici qu'est écrit tout ce que M1b-07 voulait pouvoir **montrer**. Les
/// deux régimes à un seul côté ouvert sont les seuls qui aient besoin d'être nommés : ce
/// sont eux qu'un relevé sans témoin confondait avec un câble au repos.
fn regime(compteurs: &CableCounters) -> &'static str {
    if compteurs.rendu_seul() {
        "RENDU SEUL : les trames sont jetées, rien ne s'accumule (M1b-07)"
    } else if compteurs.capture_seule() {
        "CAPTURE SEULE : l'entrée sans producteur lit du silence (M1b-07)"
    } else if compteurs.copied > 0 {
        "les deux côtés tournent : le câble transporte"
    } else if compteurs.ticks > 0 {
        "au repos : le timer tourne, aucun flux n'est ouvert"
    } else {
        "aucun tick depuis le dernier démarrage du périphérique"
    }
}

/// Les compteurs d'un câble, en trois lignes.
fn lignes_compteurs(cable: CableId, index: u32, compteurs: &CableCounters) -> String {
    let mut out = format!("  Conduit {:>2} (index pilote {index})\n", cable.0);
    let _ = writeln!(
        out,
        "      {} ticks, {} trames copiées, {} débordements",
        compteurs.ticks, compteurs.copied, compteurs.overruns
    );
    let _ = writeln!(
        out,
        "      silences : {} sans rendu, {} avant le départ du rendu ; {} ticks jetés",
        compteurs.silenced_no_render, compteurs.silenced_before_render, compteurs.discarded_ticks
    );
    let _ = writeln!(out, "      → {}", regime(compteurs));
    out
}

/// `--cable-compteurs` : le relevé de la boucle locale, sans débogueur.
///
/// `vise` restreint le relevé à un seul câble quand `--cable N` est donné ; sinon les
/// seize sont lus, comme pour `--cable-etat`.
///
/// # Pourquoi cette option existe
///
/// Les compteurs sont dans le pilote depuis M1b-07 mais ne se lisaient qu'au **débogueur
/// noyau** (`kmd_log!` est vide en release). Or l'attacher fausse précisément ce qu'on
/// mesure — 17 passes sur 20 attaché contre 20 sur 20 détaché — et coûte un redémarrage
/// de la machine virtuelle, qui ferme la session console dont l'audio a besoin. Ce relevé
/// passe par `IOCTL_KS_PROPERTY`, sans rien attacher et sans ouvrir de flux.
fn compteurs_des_cables(paths: &[String], side: FilterSide, vise: Option<u32>) -> String {
    let mut out = format!(
        "compteurs de la boucle locale, côté {} (filtres {}<n>) :\n",
        side.label(),
        side.prefix()
    );
    out.push_str(
        "  (remis à zéro à chaque démarrage du périphérique ; instantané non atomique, \
         deux ticks peuvent s'y mélanger)\n",
    );
    let numeros: Vec<u32> = match vise {
        Some(n) => vec![n],
        None => (1..=CABLE_MAX).collect(),
    };
    let mut vus = 0u32;
    for numero in numeros {
        let cable = CableId(numero);
        // Décalage de un : « Conduit 1 » est l'index 0 du pilote.
        let index = numero.saturating_sub(1);
        match ouvrir(paths, cable, side) {
            Ok(filtre) => {
                vus = vus.saturating_add(1);
                match filtre.read_counters() {
                    Ok(compteurs) => out.push_str(&lignes_compteurs(cable, index, &compteurs)),
                    Err(e) => {
                        let _ = writeln!(
                            out,
                            "  Conduit {:>2} (index pilote {index}) : lecture refusée — {e}\n      \
                             Un pilote antérieur à M1b-21 n'a pas cette propriété : vérifiez la \
                             version avec --cable-etat.",
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
    if vus == 0 {
        out.push_str(
            "Aucun filtre de topologie Conduit : le pilote n'est pas chargé sur cette \
             machine (voir docs/driver-dev.md).\n",
        );
    }
    out
}

/// Les deux lignes d'un sens : le mode en toutes lettres, puis ce qu'il a obtenu — et le
/// compte de refus **en évidence** quand il n'est pas nul.
///
/// Pure. Le mode occupe sa propre ligne parce que c'est **la** valeur qu'on vient chercher :
/// noyée en fin de ligne parmi cinq nombres, elle demanderait une seconde lecture.
fn lignes_sens(sens: StreamSide, bloc: &StreamTransport) -> String {
    let mut out = format!("      {:<8}: {}\n", sens.label(), bloc.mode_label());
    let _ = writeln!(
        out,
        "                NotificationCount {}, tampon {} octets ({} trames), {} événement(s), \
         état KS {}",
        bloc.notification_count,
        bloc.buffer_bytes,
        bloc.buffer_frames,
        bloc.notification_events,
        bloc.ks_state_label()
    );
    if bloc.refused_allocations == 0 {
        out.push_str("                aucune allocation refusée\n");
    } else {
        let _ = writeln!(
            out,
            "                ALLOCATIONS REFUSÉES : {} depuis le dernier démarrage du \
             périphérique",
            bloc.refused_allocations
        );
    }
    out
}

/// Ce que l'état du transport d'un câble **démontre**, en français.
///
/// Pure, et c'est ici qu'est écrit tout ce que le lot 0 cherche à savoir. Les quatre cas ne
/// se confondent pas, et c'est le troisième qui coûte cher à manquer : un câble scruté avec
/// des refus au compteur ne dit pas la même chose qu'un câble scruté sans aucun refus. Dans
/// le premier cas, le repli en scrutation peut être **notre** fait ; dans le second, le
/// moteur audio n'a jamais rien demandé d'autre.
fn verdict(transport: &CableTransport) -> String {
    let mut out = verdict_allocation(transport);
    // Et, dans tous les cas, ce que le pilote a déclaré savoir faire. Cette ligne est ce qui
    // distingue « le moteur audio ne veut pas de période courte » de « on ne lui a jamais
    // dit qu'on en servait » : tant que les contraintes ne sont pas déclarées,
    // `IAudioClient3::GetSharedModeEnginePeriod` n'a aucune raison d'annoncer autre chose que
    // les 10 ms du défaut de Windows, et aucune conclusion sur la latence ne tient.
    if transport.contraintes_declarees() {
        out.push_str(
            "      → contraintes de taille de paquet DÉCLARÉES des deux côtés : une période \
             plus courte que 10 ms est demandable\n",
        );
    } else {
        out.push_str(
            "      → contraintes de taille de paquet NON déclarées des deux côtés : le \
             moteur audio ne peut annoncer que 10 ms\n        Ne concluez rien sur la \
             latence avant d'avoir regardé pourquoi la pose a échoué (lignes ci-dessus, et \
             journal Système sous la source conduit_kmd).\n",
        );
    }
    out
}

/// Ce que le **mode d'allocation** du câble démontre : les quatre cas du lot 0.
fn verdict_allocation(transport: &CableTransport) -> String {
    let refus = transport.refused_total();
    if transport.notifications_obtenues() {
        let sens = if transport.render.notifie() {
            if transport.capture.notifie() {
                "des deux côtés"
            } else {
                "côté rendu"
            }
        } else {
            "côté capture"
        };
        let mut out = format!(
            "      → NOTIFICATIONS OBTENUES {sens} : un paquet WaveRT peut exister sur ce flux\n"
        );
        if refus > 0 {
            let _ = writeln!(
                out,
                "        (mais {refus} allocation(s) ont été refusées : l'autre sens, ou une \
                 tentative antérieure, s'est vu dire non)"
            );
        }
        out
    } else if refus > 0 {
        format!(
            "      → SCRUTATION, et {refus} allocation(s) REFUSÉES : le repli du moteur audio \
             peut être NOTRE fait\n        Un refus le fait retomber en scrutation sans une ligne \
             d'erreur. Ne concluez pas que Windows ne veut pas de notifications avant d'avoir \
             regardé pourquoi nous avons dit non.\n"
        )
    } else if transport.render.buffer_bytes > 0 || transport.capture.buffer_bytes > 0 {
        "      → SCRUTATION des deux côtés, aucun refus : le moteur audio n'a JAMAIS demandé \
         de notifications sur ce câble\n"
            .to_string()
    } else {
        "      → aucun tampon alloué : ce relevé ne prouve rien. Relancez-le pendant qu'une \
         passe tourne.\n"
            .to_string()
    }
}

/// La ligne des contraintes de taille de paquet d'un sens : ce que la pose de
/// `DEVPKEY_KsAudio_PacketSize_Constraints2` a donné au dernier démarrage du périphérique.
///
/// Pure. Elle **précède** le mode d'allocation dans le relevé, parce qu'elle en est la
/// cause possible : un moteur audio qui n'a jamais eu que des périodes de 10 ms n'a pas
/// forcément choisi, il peut n'avoir jamais rien su.
///
/// Le `NTSTATUS` n'est écrit que quand il désigne un échec : ni un succès (zéro) ni une pose
/// non tentée n'apprennent rien de plus que leur libellé.
fn ligne_contraintes(sens: StreamSide, transport: &CableTransport) -> String {
    let status = transport.constraints(sens);
    let libelle = transport.constraints_label(sens);
    if status == 0 || status == CONSTRAINTS_NON_TENTEE {
        format!("      {:<8}: {libelle}\n", sens.label())
    } else {
        format!("      {:<8}: {libelle} ({status:#010x})\n", sens.label())
    }
}

/// L'état du transport d'un câble : la pose des contraintes, deux sens, puis le verdict.
fn lignes_transport(cable: CableId, index: u32, transport: &CableTransport) -> String {
    let mut out = format!("  Conduit {:>2} (index pilote {index})\n", cable.0);
    out.push_str("    contraintes de taille de paquet (posées au démarrage du périphérique)\n");
    for sens in StreamSide::ALL {
        out.push_str(&ligne_contraintes(sens, transport));
    }
    out.push_str("    transport du flux courant\n");
    for sens in StreamSide::ALL {
        out.push_str(&lignes_sens(sens, transport.side(sens)));
    }
    out.push_str(&verdict(transport));
    out
}

/// Les quatre lignes de paquets d'un sens : ce que le flux courant expose, les
/// `QueryInterface` reçus, les appels par méthode, puis l'IRQL et les horodatages.
///
/// Pure. L'exposition occupe sa propre ligne, comme le mode d'allocation de [`lignes_sens`]
/// et pour la même raison : c'est **la** valeur qu'on vient chercher.
fn lignes_sens_paquets(sens: StreamSide, bloc: &StreamPackets) -> String {
    let mut out = format!("      {:<8}: {}\n", sens.label(), bloc.exposure_label());
    let _ = writeln!(
        out,
        "                QueryInterface de paquets : {} reçus, {} rendus",
        bloc.queries, bloc.queries_granted
    );
    // « appels », et non plus « appels REFUSÉS » : depuis le lot 3 le pilote **sert** les
    // quatre méthodes, et ce compteur — le même qu'au lot 2 — ne compte plus des refus.
    // Le libellé du statut est sur la ligne du verdict, qui a l'exposition sous les yeux.
    let _ = writeln!(
        out,
        "                appels : SetWritePacket {}, GetReadPacket {}, GetPacketCount \
         {}, GetOutputStreamPresentationPosition {}",
        bloc.set_write_packet, bloc.get_read_packet, bloc.packet_count, bloc.presentation_position
    );
    if bloc.emprunte() {
        let _ = writeln!(
            out,
            "                IRQL dernier {}, max {} (PASSIVE_LEVEL = 0) ; QPC premier {}, \
             dernier {}",
            bloc.irql_last, bloc.irql_max, bloc.first_qpc, bloc.last_qpc
        );
    }
    // Combien des SetWritePacket ont été refusés, par cause : c'est le refus qui pousse le
    // client à se recaler par GetPacketCount, donc le déclencheur du glissement corrigé le
    // 2026-09-12. On ne l'imprime que si le client a réellement écrit des paquets.
    if bloc.set_write_packet > 0 {
        let _ = writeln!(
            out,
            "                refus SetWritePacket : {} en retard (DATA_LATE_ERROR), {} \
             débordement (DATA_OVERRUN)",
            bloc.set_write_late, bloc.set_write_overrun
        );
    }
    // Le dernier GetPacketCount servi, et la démonstration du correctif : le compte rendu est
    // plafonné à ce que le client a fourni ; `packets_reached_at_last_count` est ce que la
    // seule position aurait annoncé, et l'écart est le glissement que le plafond évite.
    if bloc.packets_reached_at_last_count > 0 || bloc.last_packet_count_returned > 0 {
        let ecart = bloc
            .packets_reached_at_last_count
            .saturating_sub(bloc.last_packet_count_returned);
        let dernier_ecrit = if bloc.last_write_at_last_count == u64::MAX {
            "aucun".to_owned()
        } else {
            bloc.last_write_at_last_count.to_string()
        };
        let _ = writeln!(
            out,
            "                dernier GetPacketCount : rendu {}, position atteignait {} \
             paquet(s), dernier écrit {dernier_ecrit} — écart évité {ecart} paquet(s)",
            bloc.last_packet_count_returned, bloc.packets_reached_at_last_count
        );
    }
    out
}

/// Ce que le relevé de paquets d'un câble **démontre**, en français.
///
/// Pure, et c'est ici qu'est écrit ce que le lot 2 cherche à savoir : le moteur audio
/// emprunte-t-il les interfaces du mode paquets quand on les lui expose ? Les six cas ne se
/// confondent pas, et c'est le premier (un appel reçu) et le cinquième (exposé, obtenu,
/// jamais emprunté) qui tranchent.
///
/// # Ce que les `QueryInterface` ne prouvent pas, et la ligne qui le disait de travers
///
/// Le relevé du lot 2 affirmait, à `PacketMode = 0` avec des demandes non nulles, que « le
/// moteur audio a DEMANDÉ les IID de paquets » et que « notre silence est une cause plausible
/// de la scrutation ». **La mesure l'a réfuté.** Les `QueryInterface` sur les deux IID sont
/// la sonde **systématique** de PortCls à la création de tout flux — deux par flux, quel que
/// soit le client, y compris quand le chemin scruté est ensuite pris. Ils comptent des
/// créations de flux, pas des intentions du moteur audio, et aucune conclusion sur la
/// scrutation ne peut s'y appuyer. La ligne dit désormais ce qu'elle voit, et rien de plus.
///
/// # Et la ligne du lot 2 qui n'est plus vraie
///
/// « Le moteur audio EMPRUNTE un chemin que nous ne servons pas » décrivait le lot 2, qui
/// exposait sans servir. Depuis le lot 3 les quatre méthodes servent : des appels reçus
/// **par un flux qui expose** sont des appels honorés, et c'est la conformité voulue, pas
/// un danger. Le verdict distingue donc deux situations que le compteur seul confond :
///
/// - des appels alors que **rien n'est exposé** (`PacketMode = 0`) : impossible, PortCls
///   n'a jamais eu l'adresse des têtes satellites. Un compteur non nul y serait un bogue
///   d'exposition, et le relevé le dit ainsi plutôt que de le taire ;
/// - des appels **sur une interface exposée** : servis, avec l'IRQL qui vérifie le
///   `PASSIVE_LEVEL` que `portcls.h` promet.
///
/// Ce que ce relevé ne sait **pas** dire est le détail des statuts rendus : le contrat
/// d'échange (`conduit_kmd_core::config::StreamPackets`) compte des appels par méthode, pas
/// des `NTSTATUS` par appel, et le lot 3 ne l'élargit pas. Un `STATUS_DATA_LATE_ERROR` rendu
/// à un client en retard se lit dans le journal du pilote en debug, pas ici. La ligne le dit
/// plutôt que de laisser croire que tout appel compté a réussi.
fn verdict_paquets(paquets: &CablePackets) -> String {
    let demandes = paquets.queries_total();
    let appels = paquets.appels_total();
    if appels > 0 && !paquets.mode_actif() {
        return format!(
            "      → {appels} APPEL(S) alors que RIEN n'est exposé (PacketMode = 0) : \
             impossible par construction.\n        PortCls n'a jamais eu l'adresse des têtes \
             satellites ; un compteur non nul ici est un bogue d'exposition du pilote, pas \
             une mesure. Signalez-le tel quel.\n"
        );
    }
    if appels > 0 {
        return format!(
            "      → {appels} APPEL(S) SERVI(S), IRQL max {} (PASSIVE_LEVEL = 0) : le moteur \
             audio emprunte le mode paquets, et nous le servons.\n        C'est la conformité \
             visée par le lot 3. Le détail des statuts n'est pas dans ce relevé — le contrat \
             compte des appels, pas des NTSTATUS : un refus légitime (DATA_LATE_ERROR, \
             DATA_OVERRUN) se lit dans le journal du pilote en debug.\n",
            paquets.irql_max()
        );
    }
    if !paquets.mode_actif() {
        if demandes == 0 {
            return "      → rien n'est exposé (PacketMode = 0) et 0 QueryInterface reçu : \
                    aucun flux n'a été créé depuis le dernier démarrage du \
                    périphérique\n        PortCls sonde les deux IID de paquets à la \
                    création de chaque flux, deux par flux : leur absence dit qu'il n'y a \
                    pas eu de flux, rien de plus. Relancez ce relevé pendant qu'une passe \
                    tourne.\n"
                .to_string();
        }
        return format!(
            "      → rien n'est exposé (PacketMode = 0) et {demandes} QueryInterface reçus : \
             sonde de PortCls à la création de chaque flux, deux par flux\n        Ce compte \
             ne dit rien du moteur audio — PortCls interroge les deux IID de paquets sur tout \
             flux qu'il crée, quel que soit le client et quel que soit le chemin ensuite \
             pris. Mesuré : le chemin scruté les produit aussi.\n"
        );
    }
    if !paquets.exposition_courante() {
        return "      → PacketMode = 1, mais aucun flux ouvert n'expose d'interface de \
                paquets\n        Soit aucun flux ne tourne (relancez pendant une passe), soit \
                les flux courants ont été ouverts avant le dernier réglage — le paramètre est \
                lu au démarrage du périphérique.\n"
            .to_string();
    }
    if demandes == 0 {
        return "      → interfaces EXPOSÉES, 0 QueryInterface : le moteur audio ne cherche même \
                pas le mode paquets sur ce flux\n"
            .to_string();
    }
    format!(
        "      → interfaces EXPOSÉES et SERVIES, {demandes} QueryInterface dont {} rendus, 0 \
         appel : le moteur audio les obtient et NE LES EMPRUNTE PAS\n        Rien de cassé : \
         les méthodes servent, personne ne les appelle. Le moteur partagé scrute, c'est le \
         client exclusif événementiel qui les emprunte.\n",
        paquets.queries_granted_total()
    )
}

/// Le relevé de paquets d'un câble : le mode effectif, deux sens, puis le verdict.
fn lignes_paquets(paquets: &CablePackets) -> String {
    let mut out = format!(
        "    mode paquets : PacketMode = {} ({})\n",
        paquets.packet_mode,
        if paquets.mode_actif() {
            "interfaces EXPOSÉES et servies — servi, 1 par défaut depuis la campagne \
             Verifier du 2026-09-10"
        } else {
            "rien n'est exposé : le repli de diagnostic, plus le défaut"
        }
    );
    for sens in StreamSide::ALL {
        out.push_str(&lignes_sens_paquets(sens, paquets.side(sens)));
    }
    out.push_str(&verdict_paquets(paquets));
    out
}

/// `--cable-transport` : ce que le moteur audio a demandé, sans débogueur.
///
/// `vise` restreint le relevé à un seul câble quand `--cable N` est donné ; sinon les seize
/// sont lus, comme pour `--cable-etat`.
///
/// # Pourquoi cette option existe
///
/// Un paquet WaveRT n'existe que sur un tampon alloué par `AllocateBufferWithNotification`.
/// Tous les journaux du dépôt montrent le moteur audio allouant **sans** notifications — 45
/// allocations, aucun `RegisterNotificationEvent` — et l'audio passant quand même : il
/// scrute. Mais ces journaux sont anciens, pris débogueur attaché, peut-être en session 0 ;
/// or attacher le débogueur fausse ce qu'on mesure (17 passes sur 20 attaché contre 20 sur
/// 20 détaché) et coûte un redémarrage, qui ferme la session console dont l'audio a besoin.
/// Ce relevé passe par `IOCTL_KS_PROPERTY`, sans rien attacher et sans ouvrir de flux.
fn transport_des_cables(paths: &[String], side: FilterSide, vise: Option<u32>) -> String {
    let mut out = format!(
        "état du transport WaveRT, filtres ouverts côté {} ({}<n>) :\n",
        side.label(),
        side.prefix()
    );
    out.push_str(
        "  (le côté choisit le filtre, pas le sens : les DEUX sens viennent dans chaque \
         réponse)\n",
    );
    out.push_str(
        "  (remis à zéro à chaque démarrage du périphérique ; instantané non atomique, les \
         compteurs de refus sont lus hors verrou et les deux sens l'un après l'autre)\n",
    );
    out.push_str(
        "  (le relevé du mode paquets suit celui du transport, sur le même filtre : \
         PacketMode se règle dans la clé Device Parameters du périphérique et n'est lu \
         qu'au démarrage de celui-ci)\n",
    );
    let numeros: Vec<u32> = match vise {
        Some(n) => vec![n],
        None => (1..=CABLE_MAX).collect(),
    };
    let mut vus = 0u32;
    for numero in numeros {
        let cable = CableId(numero);
        // Décalage de un : « Conduit 1 » est l'index 0 du pilote.
        let index = numero.saturating_sub(1);
        match ouvrir(paths, cable, side) {
            Ok(filtre) => {
                vus = vus.saturating_add(1);
                match filtre.read_transport() {
                    Ok(transport) => out.push_str(&lignes_transport(cable, index, &transport)),
                    Err(e) => {
                        let _ = writeln!(
                            out,
                            "  Conduit {:>2} (index pilote {index}) : lecture refusée — {e}\n      \
                             Un pilote antérieur au lot 0 du mode paquets n'a pas cette \
                             propriété, et un pilote antérieur aux contraintes de taille de \
                             paquet la rend plus courte de huit octets : vérifiez la version \
                             avec --cable-etat.",
                            cable.0
                        );
                    }
                }
                // Le relevé de paquets suit celui du transport, sur le même filtre déjà
                // ouvert : les deux répondent à la même question — le moteur audio peut-il
                // faire des paquets sur ce câble, et le veut-il ? — et les lire séparément
                // obligerait à comparer deux sorties.
                match filtre.read_packets() {
                    Ok(paquets) => out.push_str(&lignes_paquets(&paquets)),
                    Err(e) => {
                        let _ = writeln!(
                            out,
                            "    mode paquets : lecture refusée — {e}\n      Un pilote \
                             antérieur au lot 2 du mode paquets n'a pas cette propriété : \
                             vérifiez la version avec --cable-etat."
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
    if vus == 0 {
        out.push_str(
            "Aucun filtre de topologie Conduit : le pilote n'est pas chargé sur cette \
             machine (voir docs/driver-dev.md).\n",
        );
    }
    out
}

/// `--cable-set` (et `--cable-chrono`) : écrit l'état, l'affiche avant et après, et
/// chronomètre éventuellement les endpoints.
///
/// `constate` est l'état du privilège **après** la tentative d'armement : il n'entre
/// dans le compte rendu qu'en cas de refus, et c'est alors le seul renseignement qui
/// dise si c'est le compte ou le pilote qu'il faut regarder.
fn ecrire(
    paths: &[String],
    cable: CableId,
    side: FilterSide,
    vise: EtatCable,
    args: &Args,
    constate: Option<EtatPrivilege>,
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
        let note = note_privilege(&e, constate);
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
    constate: Option<EtatPrivilege>,
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
        // La base est l'état **relu**, pas un état fabriqué : sur un câble qui n'est pas
        // au format d'usine, chaque entrée se ferait sinon refuser sur ses canaux plutôt
        // que sur le contrôle qu'elle vise.
        let charge = mauvaise.payload(avant);
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
        let _ = writeln!(
            out,
            "    Note : les {total} refus sont des ERROR_PRIVILEGE_NOT_HELD. Le pilote contrôle \
             le privilège AVANT de valider le contenu, par conception : cette batterie n'a donc \
             éprouvé AUCUNE des validations qu'elle vise.\n{}",
            diagnostic_privilege(constate)
        );
    }
    Ok((out, refusees == total && inchange))
}

/// Le mot à ajouter quand un refus d'écriture est un `ERROR_PRIVILEGE_NOT_HELD` (1314).
///
/// **L'ancien conseil — « relancez depuis un processus élevé » — a été démenti par la
/// mesure** : l'élévation ne suffit pas, et `LocalSystem` par tâche planifiée en
/// `/rl HIGHEST` non plus. Les trois jetons essayés portaient bien
/// `SeLoadDriverPrivilege`, mais **désactivé**, et `SeSinglePrivilegeCheck` l'exige
/// actif. Ce que le message doit donc porter, c'est l'état **constaté** du privilège :
/// c'est lui, et rien d'autre, qui sépare « mauvais compte » de « bogue du pilote ».
fn note_privilege(erreur: &CableConfigError, constate: Option<EtatPrivilege>) -> String {
    match erreur {
        CableConfigError::Requete { erreur, .. } if erreur.win32() == Some(1314) => format!(
            "    L'écriture exige SeLoadDriverPrivilege ACTIF dans le jeton de l'appelant \
             (SeSinglePrivilegeCheck, côté pilote) — le détenir ne suffit pas.\n{}",
            diagnostic_privilege(constate)
        ),
        _ => String::new(),
    }
}

/// Ce que l'état constaté du privilège permet de conclure d'un refus 1314.
///
/// Pure, et volontairement affirmative : chacun des quatre cas désigne **un** suspect.
fn diagnostic_privilege(constate: Option<EtatPrivilege>) -> &'static str {
    match constate {
        None => {
            "    État du privilège dans ce processus : non relevé. Relancez avec \
             --cable-privilege pour le connaître ; sans lui, ce refus ne peut être attribué ni \
             au compte ni au pilote."
        }
        Some(EtatPrivilege::Absent) => {
            "    État constaté : absent du jeton. C'est le COMPTE : il n'est pas administrateur, \
             et aucune élévation ne l'y ajoutera. Le pilote est hors de cause."
        }
        Some(EtatPrivilege::Desactive) => {
            "    État constaté : présent mais désactivé — l'armement n'a pas pris. C'est encore \
             le jeton, pas le pilote ; voir le bloc d'armement ci-dessus."
        }
        Some(EtatPrivilege::Actif) => {
            "    État constaté : ACTIF. Le compte est hors de cause : c'est le PILOTE qui refuse \
             un appelant privilégié. Relevez la trace de portcls::config dans la VM."
        }
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

    /// Le régime que le relevé nomme, pour chacune des cinq situations — et surtout la
    /// différence entre « rendu seul » et « au repos », que rien ne distinguait avant
    /// M1b-07 et que rien ne montrait sans débogueur avant M1b-21.
    #[test]
    fn le_releve_nomme_le_regime_que_les_compteurs_demontrent() {
        let rendu_seul = CableCounters {
            ticks: 100,
            discarded_ticks: 100,
            ..CableCounters::new(0)
        };
        assert!(regime(&rendu_seul).contains("RENDU SEUL"));

        let capture_seule = CableCounters {
            ticks: 100,
            silenced_no_render: 4_800,
            ..CableCounters::new(0)
        };
        assert!(regime(&capture_seule).contains("CAPTURE SEULE"));

        let boucle = CableCounters {
            ticks: 100,
            copied: 480_000,
            silenced_before_render: 96,
            ..CableCounters::new(0)
        };
        assert!(regime(&boucle).contains("transporte"));

        let repos = CableCounters {
            ticks: 100,
            ..CableCounters::new(0)
        };
        assert!(regime(&repos).contains("repos"));
        assert_ne!(
            regime(&repos),
            regime(&rendu_seul),
            "un câble au repos et un rendu seul doivent se lire différemment : c'est \
             exactement la confusion que M1b-07 a levée"
        );

        let neuf = CableCounters::new(0);
        assert!(regime(&neuf).contains("aucun tick"));
    }

    /// Le relevé imprime les six compteurs, le câble, son index, et le régime.
    #[test]
    fn les_lignes_de_compteurs_portent_les_six_valeurs() {
        let compteurs = CableCounters {
            cable: 2,
            reserved: 0,
            ticks: 1_111,
            copied: 22_222,
            silenced_no_render: 333,
            silenced_before_render: 44,
            discarded_ticks: 5_555_555,
            overruns: 6,
        };
        let texte = lignes_compteurs(CableId(3), 2, &compteurs);
        for attendu in [
            "Conduit  3",
            "index pilote 2",
            "1111",
            "22222",
            "333",
            "44",
            "5555555",
            "6",
        ] {
            assert!(
                texte.contains(attendu),
                "« {attendu} » absent de :\n{texte}"
            );
        }
    }

    /// Un transport où le rendu a obtenu des notifications et la capture scrute.
    fn transport_mixte() -> CableTransport {
        use conduit_backend_wasapi::cable::{AllocationMode, KsRunState};

        CableTransport {
            render: StreamTransport {
                mode: AllocationMode::Notifications.code(),
                notification_count: 2,
                buffer_bytes: 1_920,
                buffer_frames: 480,
                notification_events: 1,
                ks_state: KsRunState::Run.code(),
                refused_allocations: 0,
            },
            capture: StreamTransport {
                mode: AllocationMode::Polling.code(),
                notification_count: 0,
                buffer_bytes: 3_840,
                buffer_frames: 960,
                notification_events: 0,
                ks_state: KsRunState::Run.code(),
                refused_allocations: 0,
            },
            ..CableTransport::new(2)
        }
    }

    /// Le verdict que le relevé prononce, pour chacune des quatre situations — et surtout la
    /// différence entre « scrutation sans refus » et « scrutation avec refus », qui est
    /// exactement la question du lot 0.
    ///
    /// Confondre les deux ferait conclure « Windows ne veut pas de paquets » d'un câble où
    /// c'est nous qui avons dit non. Aucun journal ne le dirait : un refus fait retomber le
    /// moteur en scrutation **sans une ligne d'erreur**.
    #[test]
    fn le_verdict_distingue_la_scrutation_choisie_de_la_scrutation_subie() {
        use conduit_backend_wasapi::cable::AllocationMode;

        let obtenu = verdict(&transport_mixte());
        assert!(obtenu.contains("NOTIFICATIONS OBTENUES"), "{obtenu}");
        assert!(obtenu.contains("rendu"), "{obtenu}");

        // Scrutation des deux côtés, aucun refus : le moteur n'a jamais rien demandé.
        let scrute = CableTransport {
            render: StreamTransport {
                mode: AllocationMode::Polling.code(),
                buffer_bytes: 1_920,
                buffer_frames: 480,
                ..StreamTransport::new()
            },
            capture: StreamTransport {
                mode: AllocationMode::Polling.code(),
                buffer_bytes: 1_920,
                buffer_frames: 480,
                ..StreamTransport::new()
            },
            ..CableTransport::new(0)
        };
        let choisie = verdict(&scrute);
        assert!(choisie.contains("JAMAIS demandé"), "{choisie}");

        // La même scrutation, mais avec des refus : le repli peut être notre fait.
        let subie = verdict(&CableTransport {
            render: StreamTransport {
                refused_allocations: 4,
                ..scrute.render
            },
            ..scrute
        });
        assert!(subie.contains("REFUSÉES"), "{subie}");
        assert!(subie.contains("NOTRE fait"), "{subie}");
        assert_ne!(
            choisie, subie,
            "une scrutation subie et une scrutation choisie doivent se lire différemment : \
             c'est toute la question du lot"
        );

        // Aucun tampon : le relevé le dit au lieu de laisser conclure.
        let vide = verdict(&CableTransport::new(0));
        assert!(vide.contains("ne prouve rien"), "{vide}");

        // Les quatre verdicts sont distincts.
        let tous = [obtenu, choisie, subie, vide];
        for (rang, texte) in tous.iter().enumerate() {
            assert!(!texte.is_empty());
            assert!(
                !tous.iter().skip(rang + 1).any(|autre| autre == texte),
                "deux verdicts identiques : {texte}"
            );
        }
    }

    /// Le verdict dit **toujours** si les contraintes de taille de paquet ont été déclarées.
    ///
    /// Sans cette ligne, un relevé « scrutation, aucun refus » laisserait conclure que
    /// Windows ne veut pas de période courte, alors qu'il se peut qu'on ne lui ait jamais dit
    /// qu'on savait en servir — l'exacte symétrie de l'erreur que le compteur de refus
    /// évitait déjà pour les notifications.
    #[test]
    fn le_verdict_dit_si_les_contraintes_de_paquet_sont_declarees() {
        // Un câble tout neuf : pose non tentée, donc « NON déclarées ».
        let jamais = verdict(&CableTransport::new(0));
        assert!(jamais.contains("NON déclarées"), "{jamais}");
        assert!(jamais.contains("que 10 ms"), "{jamais}");

        // Les deux sens posés : la période courte est demandable.
        let posees = verdict(&CableTransport {
            constraints_render: 0,
            constraints_capture: 0,
            ..transport_mixte()
        });
        assert!(posees.contains("DÉCLARÉES des deux côtés"), "{posees}");

        // Un seul côté posé ne suffit pas : l'autre filtre reste à 10 ms.
        let moitie = verdict(&CableTransport {
            constraints_render: 0,
            constraints_capture: 0xC000_000D,
            ..transport_mixte()
        });
        assert!(moitie.contains("NON déclarées"), "{moitie}");
    }

    /// Les deux lignes de pose : le libellé de chaque sens, et le `NTSTATUS` **seulement**
    /// quand il désigne un échec.
    #[test]
    fn les_lignes_de_contraintes_portent_le_status_du_seul_echec() {
        let transport = CableTransport {
            constraints_render: 0,
            constraints_capture: 0xC000_000D,
            ..transport_mixte()
        };
        let rendu = ligne_contraintes(StreamSide::Render, &transport);
        assert!(rendu.contains("DÉCLARÉES"), "{rendu}");
        assert!(!rendu.contains("0x"), "un succès n'a pas de code : {rendu}");

        let capture = ligne_contraintes(StreamSide::Capture, &transport);
        assert!(capture.contains("REFUSÉES"), "{capture}");
        assert!(capture.contains("0xc000000d"), "{capture}");

        // Une pose non tentée se lit comme telle, et sans code : la sentinelle n'est pas un
        // `NTSTATUS` du noyau et l'afficher n'apprendrait rien.
        let neuf = CableTransport::new(0);
        assert_eq!(neuf.constraints(StreamSide::Render), CONSTRAINTS_NON_TENTEE);
        let ligne = ligne_contraintes(StreamSide::Render, &neuf);
        assert!(ligne.contains("non tentées"), "{ligne}");
        assert!(!ligne.contains("0x"), "{ligne}");
    }

    /// Le relevé imprime, pour chaque sens, le mode **en toutes lettres**, les cinq valeurs
    /// et le compte de refus.
    #[test]
    fn les_lignes_de_transport_portent_les_deux_sens_et_les_refus() {
        use conduit_backend_wasapi::cable::AllocationMode;

        let transport = CableTransport {
            render: StreamTransport {
                refused_allocations: 42,
                ..transport_mixte().render
            },
            ..transport_mixte()
        };
        let texte = lignes_transport(CableId(3), 2, &transport);
        for attendu in [
            "Conduit  3",
            "index pilote 2",
            "rendu",
            "capture",
            AllocationMode::Notifications.label(),
            AllocationMode::Polling.label(),
            "NotificationCount 2",
            "1920 octets",
            "480 trames",
            "3840 octets",
            "960 trames",
            "RUN",
            "ALLOCATIONS REFUSÉES : 42",
            "aucune allocation refusée",
        ] {
            assert!(
                texte.contains(attendu),
                "« {attendu} » absent de :\n{texte}"
            );
        }
        // Le mode est sur sa propre ligne : c'est la valeur qu'on vient chercher, elle ne
        // doit pas se noyer parmi cinq nombres.
        assert!(
            texte.lines().any(|l| l
                .trim_end()
                .ends_with(AllocationMode::Notifications.label())),
            "{texte}"
        );
    }

    /// Le verdict du relevé de paquets, pour chacune des sept situations.
    ///
    /// Les deux premières sont celles que la mesure a corrigées : un `QueryInterface` de
    /// paquets est la **sonde de PortCls** à la création de chaque flux, deux par flux, quel
    /// que soit le client. Le relevé disait « le moteur audio a DEMANDÉ les IID » et « notre
    /// silence est une cause plausible de la scrutation » ; il ne le dit plus, et ce test
    /// interdit que la formule revienne.
    ///
    /// La sixième est celle que le **lot 3** a retournée : des appels reçus ne sont plus des
    /// refus mais des appels servis, et la septième — des appels sans rien d'exposé —
    /// désigne désormais un bogue du pilote plutôt qu'une mesure.
    #[test]
    fn le_verdict_des_paquets_distingue_les_sept_situations() {
        use conduit_backend_wasapi::cable::PacketExposure;

        // 1. Rien d'exposé, aucune sonde reçue : aucun flux n'a été créé, rien de plus.
        let politique = verdict_paquets(&CablePackets::new(0));
        assert!(politique.contains("0 QueryInterface reçu"), "{politique}");
        assert!(politique.contains("aucun flux"), "{politique}");

        // 2. Rien d'exposé, des sondes reçues : un constat neutre, aucune conclusion sur le
        //    moteur audio.
        let demande = verdict_paquets(&CablePackets {
            render: StreamPackets {
                exposure: PacketExposure::NotExposed.code(),
                queries: 4,
                ..StreamPackets::new()
            },
            ..CablePackets::new(0)
        });
        assert!(demande.contains("4 QueryInterface reçus"), "{demande}");
        assert!(demande.contains("sonde de PortCls"), "{demande}");
        assert!(demande.contains("deux par flux"), "{demande}");
        // La conclusion réfutée par la mesure ne doit revenir sous aucune forme.
        for interdit in ["DEMANDÉ", "cause plausible", "PacketMode = 1"] {
            assert!(
                !demande.contains(interdit),
                "« {interdit} » a reparu dans :\n{demande}"
            );
        }

        // 3. Mode actif mais aucun flux n'expose : le relevé le dit au lieu de laisser
        //    conclure.
        let trop_tot = verdict_paquets(&CablePackets {
            packet_mode: 1,
            ..CablePackets::new(0)
        });
        assert!(trop_tot.contains("aucun flux"), "{trop_tot}");

        // 4. Exposé, jamais demandé : le moteur ne cherche même pas.
        let ignore = verdict_paquets(&CablePackets {
            packet_mode: 1,
            render: StreamPackets {
                exposure: PacketExposure::Output.code(),
                ..StreamPackets::new()
            },
            ..CablePackets::new(0)
        });
        assert!(ignore.contains("0 QueryInterface"), "{ignore}");

        // 5. Exposé, obtenu, jamais emprunté : la lecture que l'énoncé du lot demande.
        let obtenu = verdict_paquets(&CablePackets {
            packet_mode: 1,
            render: StreamPackets {
                exposure: PacketExposure::Output.code(),
                queries: 2,
                queries_granted: 2,
                ..StreamPackets::new()
            },
            ..CablePackets::new(0)
        });
        assert!(obtenu.contains("EXPOSÉES"), "{obtenu}");
        assert!(obtenu.contains("0 appel"), "{obtenu}");

        // 6. Emprunté et **servi** : la conformité que le lot 3 vise. La ligne du lot 2 —
        //    « EMPRUNTE un chemin que nous ne servons pas » — n'est plus vraie, et ce test
        //    interdit qu'elle revienne.
        let emprunte = verdict_paquets(&CablePackets {
            packet_mode: 1,
            render: StreamPackets {
                exposure: PacketExposure::Output.code(),
                queries: 2,
                queries_granted: 2,
                set_write_packet: 9,
                irql_max: 0,
                ..StreamPackets::new()
            },
            ..CablePackets::new(0)
        });
        assert!(emprunte.contains("9 APPEL(S) SERVI(S)"), "{emprunte}");
        assert!(emprunte.contains("IRQL max 0"), "{emprunte}");
        for interdit in ["REFUSÉ", "STATUS_NOT_SUPPORTED", "que nous ne servons pas"] {
            assert!(
                !emprunte.contains(interdit),
                "« {interdit} » a reparu dans :\n{emprunte}"
            );
        }

        // 7. Des appels alors que rien n'est exposé : impossible par construction, donc un
        //    bogue du pilote — et le relevé le dit au lieu de conclure sur le moteur audio.
        let impossible = verdict_paquets(&CablePackets {
            render: StreamPackets {
                exposure: PacketExposure::NotExposed.code(),
                get_read_packet: 3,
                ..StreamPackets::new()
            },
            ..CablePackets::new(0)
        });
        assert!(
            impossible.contains("impossible par construction"),
            "{impossible}"
        );
        assert!(impossible.contains("bogue d'exposition"), "{impossible}");

        // Les sept verdicts sont distincts : deux situations différentes ne doivent pas se
        // lire pareil.
        let tous = [
            politique, demande, trop_tot, ignore, obtenu, emprunte, impossible,
        ];
        for (rang, texte) in tous.iter().enumerate() {
            assert!(!texte.is_empty());
            assert!(
                !tous.iter().skip(rang + 1).any(|autre| autre == texte),
                "deux verdicts identiques : {texte}"
            );
        }
    }

    /// Le relevé de paquets imprime le mode effectif, l'exposition de chaque sens en toutes
    /// lettres, les `QueryInterface` et les appels par méthode.
    #[test]
    fn les_lignes_de_paquets_portent_le_mode_les_deux_sens_et_les_appels() {
        use conduit_backend_wasapi::cable::PacketExposure;

        let paquets = CablePackets {
            cable: 2,
            packet_mode: 1,
            render: StreamPackets {
                exposure: PacketExposure::Output.code(),
                irql_last: 0,
                irql_max: 0,
                reserved: 0,
                set_write_packet: 13,
                get_read_packet: 0,
                packet_count: 5,
                presentation_position: 7,
                queries: 29,
                queries_granted: 29,
                first_qpc: 1_000,
                last_qpc: 2_000,
                set_write_late: 3,
                set_write_overrun: 1,
                // Le client a écrit jusqu'au paquet 4 (cinq paquets fournis) ; la position
                // avait atteint sept paquets complets : le plafond a évité un saut de deux.
                last_packet_count_returned: 5,
                packets_reached_at_last_count: 7,
                last_write_at_last_count: 4,
            },
            capture: StreamPackets {
                exposure: PacketExposure::Input.code(),
                ..StreamPackets::new()
            },
        };
        let texte = lignes_paquets(&paquets);
        for attendu in [
            "PacketMode = 1",
            "rendu",
            "capture",
            PacketExposure::Output.label(),
            PacketExposure::Input.label(),
            "29 reçus, 29 rendus",
            "SetWritePacket 13",
            "GetPacketCount 5",
            "GetOutputStreamPresentationPosition 7",
            "IRQL dernier 0, max 0",
            "QPC premier 1000, dernier 2000",
            "refus SetWritePacket : 3 en retard",
            "1 débordement",
            "dernier GetPacketCount : rendu 5, position atteignait 7",
            "dernier écrit 4 — écart évité 2 paquet(s)",
        ] {
            assert!(
                texte.contains(attendu),
                "« {attendu} » absent de :\n{texte}"
            );
        }
        // L'exposition est sur sa propre ligne, comme le mode d'allocation du transport.
        assert!(
            texte
                .lines()
                .any(|l| l.trim_end().ends_with(PacketExposure::Output.label())),
            "{texte}"
        );
        // Un sens sans appel n'imprime pas de ligne d'IRQL : il n'y a rien à en dire, et une
        // ligne de zéros ferait croire à une mesure.
        let repos = lignes_paquets(&CablePackets::new(0));
        assert!(!repos.contains("IRQL"), "{repos}");
        assert!(repos.contains("PacketMode = 0"), "{repos}");
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

    /// Un refus 1314, tel que le pilote le rend.
    fn refus_1314() -> CableConfigError {
        CableConfigError::Requete {
            propriete: "KSPROPERTY_CONDUIT_CABLE_STATE",
            verbe: "SET",
            erreur: OsError::Win32(1314),
        }
    }

    #[test]
    fn la_note_de_privilege_ne_sort_que_sur_1314() {
        let note = note_privilege(&refus_1314(), Some(EtatPrivilege::Actif));
        assert!(note.contains("SeLoadDriverPrivilege"), "{note}");
        let autre = CableConfigError::Requete {
            propriete: "KSPROPERTY_CONDUIT_CABLE_STATE",
            verbe: "SET",
            erreur: OsError::Win32(87),
        };
        assert_eq!(note_privilege(&autre, Some(EtatPrivilege::Actif)), "");
        assert_eq!(note_privilege(&autre, None), "");
    }

    /// Le conseil démenti par la mesure ne doit **jamais** revenir : l'élévation ne
    /// suffisait pas, et `LocalSystem` non plus.
    #[test]
    fn plus_aucun_message_ne_conseille_l_elevation() {
        let mut messages = vec![
            note_privilege(&refus_1314(), None),
            bloc_armement(Armement::Arme),
            bloc_armement(Armement::Absent),
            bloc_armement(Armement::NonActivable { code: 1300 }),
        ];
        for etat in [
            EtatPrivilege::Absent,
            EtatPrivilege::Desactive,
            EtatPrivilege::Actif,
        ] {
            messages.push(note_privilege(&refus_1314(), Some(etat)));
            messages.push(bloc_privilege(&Ok(etat)));
        }
        for message in &messages {
            let minuscules = message.to_lowercase();
            assert!(
                !minuscules.contains("processus élevé"),
                "un message conseille encore l'élévation : {message}"
            );
        }
    }

    /// Les quatre diagnostics d'un refus 1314 désignent chacun **un** suspect, et se
    /// distinguent les uns des autres.
    #[test]
    fn le_diagnostic_d_un_refus_nomme_le_suspect() {
        let inconnu = diagnostic_privilege(None);
        assert!(inconnu.contains("--cable-privilege"), "{inconnu}");

        let absent = diagnostic_privilege(Some(EtatPrivilege::Absent));
        assert!(absent.contains("COMPTE"), "{absent}");
        assert!(absent.contains("hors de cause"), "{absent}");

        let dormant = diagnostic_privilege(Some(EtatPrivilege::Desactive));
        assert!(dormant.contains("désactivé"), "{dormant}");

        let actif = diagnostic_privilege(Some(EtatPrivilege::Actif));
        assert!(actif.contains("PILOTE"), "{actif}");
        assert!(actif.contains("portcls::config"), "{actif}");

        let tous = [inconnu, absent, dormant, actif];
        for (rang, texte) in tous.iter().enumerate() {
            assert!(!texte.is_empty());
            assert!(
                !tous.iter().skip(rang + 1).any(|autre| autre == texte),
                "deux diagnostics identiques : {texte}"
            );
        }
    }

    /// `--cable-privilege` doit dire l'état **et** ce qu'il implique, y compris quand
    /// l'interrogation du jeton a échoué.
    #[test]
    fn le_bloc_de_privilege_explique_les_trois_etats() {
        let dormant = bloc_privilege(&Ok(EtatPrivilege::Desactive));
        assert!(dormant.contains("présent mais désactivé"), "{dormant}");
        // Le point que la mesure a établi : élevé et LocalSystem sont dans ce cas.
        assert!(dormant.contains("LocalSystem"), "{dormant}");
        assert!(dormant.contains("ACTIF"), "{dormant}");

        let absent = bloc_privilege(&Ok(EtatPrivilege::Absent));
        assert!(absent.contains("absent du jeton"), "{absent}");
        assert!(absent.contains("administrateur"), "{absent}");

        let actif = bloc_privilege(&Ok(EtatPrivilege::Actif));
        assert!(actif.contains("présent et actif"), "{actif}");

        // L'échec de l'interrogation se dit, il ne se tait pas.
        let inconnu = bloc_privilege(&Err(CableConfigError::Privilege {
            appel: "OpenProcessToken",
            erreur: OsError::Win32(5),
        }));
        assert!(inconnu.contains("INCONNU"), "{inconnu}");
        assert!(inconnu.contains("OpenProcessToken"), "{inconnu}");

        // Chaque bloc finit par un saut de ligne : `pousser` compte dessus.
        for bloc in [&dormant, &absent, &actif, &inconnu] {
            assert!(bloc.ends_with('\n'), "{bloc}");
        }
    }

    /// L'issue d'un armement se traduit en état de jeton sans relire celui-ci.
    #[test]
    fn l_issue_de_l_armement_dit_l_etat_du_jeton() {
        assert_eq!(etat_apres(Armement::Arme), EtatPrivilege::Actif);
        assert_eq!(etat_apres(Armement::Absent), EtatPrivilege::Absent);
        assert_eq!(
            etat_apres(Armement::NonActivable { code: 1300 }),
            EtatPrivilege::Desactive
        );
        // Seul l'armement réussi laisse le privilège actif.
        for issue in [
            Armement::Arme,
            Armement::Absent,
            Armement::NonActivable { code: 1300 },
        ] {
            assert_eq!(etat_apres(issue).actif(), issue.arme(), "{issue}");
        }
    }

    /// Le bloc d'armement ne conseille rien quand il n'y a rien à faire, et nomme la
    /// conduite à tenir dans les deux autres cas.
    #[test]
    fn le_bloc_d_armement_ne_conseille_que_s_il_y_a_lieu() {
        assert_eq!(conseil_armement(Armement::Arme), "");
        assert!(conseil_armement(Armement::Absent).contains("administrateur"));
        assert!(conseil_armement(Armement::NonActivable { code: 1300 }).contains("rare"));

        let arme = bloc_armement(Armement::Arme);
        assert!(arme.contains("armé"), "{arme}");
        assert!(arme.ends_with('\n'), "{arme}");
        let absent = bloc_armement(Armement::Absent);
        assert!(absent.contains("administrateur"), "{absent}");
        assert!(absent.ends_with('\n'), "{absent}");
    }
}
