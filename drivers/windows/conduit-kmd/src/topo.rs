//! Miniports topologie d'un câble (driver-design.md §4) : [`TopoRender`] et
//! [`TopoCapture`], un par sens, implémentent `portcls::MiniportTopology` et
//! `portcls::AudioNodes`.
//!
//! Ils exposent le descripteur de filtre **de leur câble**
//! (`descriptors::topo_render_filter(n)`, `topo_capture_filter(n)` : broche bridge,
//! broche endpoint nommée par le GUID du câble, nœud de volume, nœud de
//! sourdine) et servent les propriétés de ces nœuds en déléguant au [`NodeState`] du bon
//! sens dans le câble. Ils implémentent aussi `portcls::JackInfo`, la propriété de
//! **filtre** `KSPROPERTY_JACK_DESCRIPTION` (M1b-03), et `portcls::CableConfig`, le jeu de
//! propriétés **privé** `KSPROPSETID_Conduit` (M1b-04), et `portcls::EventSource`, par
//! lequel part `KSEVENT_PINCAPS_JACKINFOCHANGE`. `DataRangeIntersection` reste au défaut
//! (PortCls intersecte lui-même).
//!
//! # Ce qu'`Init` retient du port, et jusqu'à quand
//!
//! `Init` ne retient du port qu'**une** chose : son `IPortEvents`, obtenu par
//! `QueryInterface` (`portcls::PortEvents::from_port`). L'enveloppe `IPortTopology` et la
//! liste de ressources sont relâchées en sortie comme avant (`Drop` des enveloppes).
//!
//! C'est le seul chemin par lequel cette interface est atteignable : elle s'obtient sur
//! l'objet **port**, que seul un miniport de topologie voit, et il y a **deux ports par
//! câble**. Le miniport la dépose donc, avec le numéro de **sa** broche endpoint, dans le
//! câble ([`Cable::attach_jack_events`]), qui porte les deux et les signale tous les deux
//! quand son état de connexion change (`portcls::JackTargets`).
//!
//! La référence COM ainsi prise est rendue par le **`Drop` du miniport**
//! ([`Cable::detach_jack_events`]) : c'est l'instant où PortCls a fini d'appeler cet objet
//! — le `Drop` suit son `Release` final — donc le premier où plus personne ne peut avoir
//! besoin du port par notre intermédiaire. La rendre plus tôt exposerait un signalement
//! concurrent à un port détruit ; plus tard, il n'y a pas de plus tard.
//!
//! Un `QueryInterface(IID_IPortEvents)` qui échoue fait **échouer `Init`**, comme la
//! topologie de SYSVAD. Le port de PortCls implémente toujours cette interface : un refus
//! voudrait dire que ce n'est pas le port qu'on croit, et continuer produirait un endpoint
//! dont l'état ne pourrait plus jamais changer sans que rien ne le dise. Un `StartDevice`
//! qui échoue proprement se diagnostique ; une interface figée, non (driver-design.md §7).
//!
//! # Les deux sens implémentent le **même** état de configuration
//!
//! `CableConfig` est implémenté à l'identique par [`TopoRender`] et [`TopoCapture`], et
//! délègue au même [`Cable`] : un câble débranché l'est de ses deux bouts. Le service
//! d'assistance (M1b-20) peut donc s'adresser à l'un ou à l'autre filtre — c'est délibéré,
//! et ça évite d'avoir à documenter « le côté rendu est celui qui commande ». Les deux
//! implémentations restent écrites en toutes lettres plutôt que factorisées par une macro :
//! `portcls::property` monomorphise ses gestionnaires **par type de miniport** (la garde de
//! vtable compare l'adresse de `T::VTBL`), et deux implémentations distinctes sont
//! exactement ce que le reste du fichier fait déjà pour `AudioNodes` et `JackInfo`.
//!
//! # Le jack : quelle broche, et quelle cartographie
//!
//! Deux valeurs seulement distinguent les deux sens, et toutes deux sont des pièges.
//!
//! **La broche.** [`JackInfo::jack_pin`] désigne la broche qui **fait face à
//! l'extérieur** — celle dont Windows tire l'endpoint, celle qui porte le nom du câble :
//! [`TOPO_RENDER_PIN_ENDPOINT`] (1) au rendu, [`TOPO_CAPTURE_PIN_ENDPOINT`] (0) à la
//! capture. Ce n'est **pas** la broche que `descriptors::bridge_pin` construit, malgré le
//! vocabulaire de la documentation Microsoft, qui appelle « bridge pin » la broche à prise.
//! Se tromper ne casse rien de visible : la propriété répond, mais par une description
//! vide, et l'endpoint garde l'état de connexion par défaut de Windows. D'où les
//! assertions `const` ci-dessous.
//!
//! **La cartographie.** `KSJACK_DESCRIPTION::ChannelMapping` doit être non nul « *only for
//! analog rendering pins* » : le masque `KSAUDIO_SPEAKER_*` du nombre de canaux **du
//! câble** au rendu ([`mapping_rendu`]), **0** à la capture.
//!
//! L'état de connexion, lui, est **par câble** et commun aux deux sens
//! ([`Cable::is_connected`]) : un câble débranché l'est de ses deux bouts.
//!
//! # Le volume est mémorisé, pas appliqué
//!
//! Les deux moitiés du mécanisme sont documentées sur [`NodeState`] et dans
//! driver-design.md §5.5 : **exposer** le nœud fait renoncer Windows à son APO logiciel,
//! **ne pas appliquer** la valeur qu'il y pousse aussitôt est ce qui rend le câble
//! transparent. Ces implémentations sont donc de purs accesseurs sur des atomiques ;
//! aucune ne touche au signal.
//!
//! # Le nombre de canaux
//!
//! [`AudioNodes::channels`] doit s'accorder avec le format des broches : depuis M1b-05,
//! c'est celui du **câble** (`descriptors::cable_format(n).channels`), entre 1 et 8, et non
//! plus une constante. Une divergence rendrait `STATUS_INVALID_PARAMETER` sur un canal
//! pourtant déclaré par le format, ou l'inverse.
//!
//! Les assertions `const` qui scellaient la valeur 2 sont devenues des **invariants sur le
//! domaine** : le plafond des canaux d'un câble ([`FrameLayout::MAX_CHANNELS`], 8) tient
//! dans ce que [`NodeState`] mémorise ([`cable::MAX_CHANNELS`]), et chaque masque de
//! haut-parleurs de [`MAPPINGS_RENDU`] porte exactement autant de bits que de canaux. Ce
//! qu'on vérifiait sur une valeur, on le vérifie sur les huit — c'est plus fort, pas moins.
//!
//! # La trace
//!
//! [`AudioNodes::trace`] est câblée sur `kmd_log!` en une ligne. Elle existe pour rendre
//! vérifiable en une minute, au premier essai en machine, le seul décalage du plan dont
//! l'erreur serait silencieuse : le `Channel` en tête de `Instance` (voir la documentation
//! de `portcls::audio`). Attendu sur un endpoint stéréo : Windows interroge le canal `0`
//! puis le canal `1`, et écrit avec le canal `-1`. Si la trace ne montre que des
//! « canal 0 », c'est le `Reserved` qu'on lit. Vide en release (`kmd_log!`).

use core::fmt;

use conduit_kmd_core::FrameLayout;
use conduit_kmd_core::config::{CableCounters, CablePackets, CableTransport};
use portcls::conduit_com::{ComRef, NtStatus, STATUS_SUCCESS};
use portcls::{
    AudioNodes, CableConfig, ConfigTrace, EventSource, EventTrace, JackInfo, JackTarget, JackTrace,
    MiniportTopology, PortEvents, PortTopology, ResourceList, Trace,
};
use portcls_sys::{
    IUnknown, KSAUDIO_SPEAKER_2POINT1, KSAUDIO_SPEAKER_5POINT0, KSAUDIO_SPEAKER_5POINT1,
    KSAUDIO_SPEAKER_7POINT0, KSAUDIO_SPEAKER_7POINT1_SURROUND, KSAUDIO_SPEAKER_MONO,
    KSAUDIO_SPEAKER_QUAD, KSAUDIO_SPEAKER_STEREO, PCFILTER_DESCRIPTOR, ULONG,
};

use crate::cable::{Cable, Direction, MAX_CHANNELS, NodeState};
use crate::descriptors::{
    PIN_COUNT, TOPO_CAPTURE_PIN_ENDPOINT, TOPO_RENDER_PIN_ENDPOINT, cable_format,
    declared_by_render_system_pin, topo_capture_filter, topo_capture_filter_0, topo_render_filter,
    topo_render_filter_0,
};
use crate::privilege;

/// Nombre de broches d'un filtre de topologie, du type que `JackInfo::pin_count` rend.
///
/// Doit valoir `PCFILTER_DESCRIPTOR::PinCount` : c'est lui qui sépare « broche existante
/// sans prise » (réponse vide, `STATUS_SUCCESS`) de « broche inexistante »
/// (`STATUS_INVALID_PARAMETER`).
const BROCHES: u32 = PIN_COUNT as u32;

/// `KSJACK_DESCRIPTION::ChannelMapping` du sens **rendu**, par nombre de canaux : le
/// masque `KSAUDIO_SPEAKER_*` de la disposition la plus courante à ce compte-là.
///
/// # Un masque faux ne casse rien de visible, et c'est le problème
///
/// Ce champ dit à Windows *quels haut-parleurs* les canaux alimentent. Un câble à six
/// canaux qui annoncerait encore une cartographie stéréo se verrait dans le nom des canaux
/// affiché par le panneau de son, dans le rangement de l'endpoint parmi les périphériques
/// surround, et nulle part ailleurs — pas dans le journal, pas dans `infverif`, pas à
/// l'écoute. D'où l'assertion `const` plus bas : **autant de bits à 1 que de canaux**,
/// pour les huit entrées.
///
/// Les choix, du plus courant au plus discutable : mono (centre), stéréo (avant
/// gauche/droit), 2.1 (avant + caisson), quadriphonie, 5.0 *surround* (avant + latéraux),
/// 5.1, 7.0 et 7.1 *surround*. Les comptes impairs sans disposition standard — 3, 5, 7 —
/// prennent celle que Windows nomme sans « point » manquant ; aucun n'est faux, tous sont
/// conventionnels.
const MAPPINGS_RENDU: [ULONG; MAX_CANAUX] = [
    KSAUDIO_SPEAKER_MONO,
    KSAUDIO_SPEAKER_STEREO,
    KSAUDIO_SPEAKER_2POINT1,
    KSAUDIO_SPEAKER_QUAD,
    KSAUDIO_SPEAKER_5POINT0,
    KSAUDIO_SPEAKER_5POINT1,
    KSAUDIO_SPEAKER_7POINT0,
    KSAUDIO_SPEAKER_7POINT1_SURROUND,
];

/// Nombre maximal de canaux d'un câble (SPEC F-03), la longueur de [`MAPPINGS_RENDU`].
const MAX_CANAUX: usize = FrameLayout::MAX_CHANNELS as usize;

/// `KSJACK_DESCRIPTION::ChannelMapping` du sens **capture** : nul, comme la documentation
/// l'exige pour toute broche qui n'est pas une broche de rendu analogique.
const MAPPING_CAPTURE: ULONG = 0;

/// Le masque de haut-parleurs de `channels` canaux ; stéréo hors domaine (repli du
/// défaut, comme `descriptors::cable_format`).
fn mapping_rendu(channels: u8) -> ULONG {
    usize::from(channels)
        .checked_sub(1)
        .and_then(|i| MAPPINGS_RENDU.get(i))
        .copied()
        .unwrap_or(KSAUDIO_SPEAKER_STEREO)
}

const _: () = {
    // La broche à prise est l'endpoint, celle qui fait face à l'extérieur — jamais la
    // broche bridge vers le filtre WaveRT (voir l'en-tête de module).
    assert!(TOPO_RENDER_PIN_ENDPOINT == 1 && TOPO_CAPTURE_PIN_ENDPOINT == 0);
    assert!(TOPO_RENDER_PIN_ENDPOINT < BROCHES && TOPO_CAPTURE_PIN_ENDPOINT < BROCHES);
    // Les deux sens ne visent pas la même broche, et c'est ce qui rend le signalement de
    // `KSEVENT_PINCAPS_JACKINFOCHANGE` faux ou juste : une `JackTarget` construite avec la
    // broche de l'autre sens partirait quand même, sans réveiller personne (voir
    // `portcls::event`). Si un jour les deux valaient le même numéro, la confusion
    // deviendrait invisible — d'où cette assertion, qui n'a l'air de rien.
    assert!(TOPO_RENDER_PIN_ENDPOINT != TOPO_CAPTURE_PIN_ENDPOINT);
    assert!(MAPPING_CAPTURE == 0);

    // Ce que M1a scellait sur la valeur 2, vérifié sur les huit : tout nombre de canaux
    // qu'un câble peut porter tient dans ce que `NodeState` mémorise…
    assert!(MAX_CANAUX <= MAX_CHANNELS);
    assert!(MAPPINGS_RENDU.len() == MAX_CANAUX && MAX_CANAUX == 8);
    // …et chaque masque de haut-parleurs porte exactement autant de bits que de canaux.
    // Motif de tranche : ni indexation ni arithmétique.
    let mut masques: &[ULONG] = &MAPPINGS_RENDU;
    let mut attendu: u32 = 1;
    while let [premier, reste @ ..] = masques {
        assert!(
            premier.count_ones() == attendu,
            "un masque de haut-parleurs ne compte pas ses canaux"
        );
        masques = reste;
        attendu = attendu.wrapping_add(1);
    }
    assert!(
        attendu as usize == MAX_CANAUX.wrapping_add(1),
        "un masque par compte"
    );
    // Le stéréo reste le repli, et il est bien à sa place dans la table.
    assert!(KSAUDIO_SPEAKER_STEREO.count_ones() == 2);

    // **Une seule vérité sur les masques** (M1b-21). `conduit_kmd_core::wavefmt` en tient
    // désormais sa propre table — c'est elle qui remplit le `dwChannelMask` des
    // `WAVEFORMATEXTENSIBLE` que le gestionnaire d'intersection rend — et elle est testée
    // en mode utilisateur, ce que celle-ci ne peut pas être. Les deux doivent dire la même
    // chose : un jack qui annonce une disposition et un format qui en annonce une autre
    // est exactement le genre de divergence qui ne se voit nulle part.
    let mut notres: &[ULONG] = &MAPPINGS_RENDU;
    let mut canal: u8 = 1;
    while let [premier, reste @ ..] = notres {
        let portable = match conduit_kmd_core::speaker_mask(canal) {
            Some(m) => m,
            None => 0,
        };
        assert!(
            portable == *premier,
            "le masque du jack et celui du WAVEFORMATEXTENSIBLE divergent"
        );
        notres = reste;
        canal = canal.wrapping_add(1);
    }
};

// ---------------------------------------------------------------------------------
// Le garde-fou de la topologie (M1b-05).
//
// `descriptors::check_cable_pins` referme l'intervalle du côté **wave** : il vérifie que la
// broche système déclarera bien la fréquence et les canaux que le registre annonce. Il ne
// dit rien du côté **topologie**, et un endpoint ne naît pas d'un filtre mais de la
// connexion des deux. Ce que la topologie déclare du format ne passe pas par une
// `KSDATARANGE` — les broches endpoint sont analogiques — mais par deux valeurs calculées à
// l'exécution : le nombre de canaux des nœuds volume et sourdine (`AudioNodes::channels`,
// que Windows lit par `BASICSUPPORT`) et la cartographie de haut-parleurs du jack
// (`JackInfo::channel_mapping`). Toutes deux ont un repli **muet** — `mapping_rendu` rend
// le masque stéréo hors domaine, `description` rend le filtre du câble 0 faute de rangée —
// et `kmd_log!` est vide en release.
//
// [`check_cable_topology`] les confronte, au démarrage, à ce que la broche wave déclare
// **réellement** (`descriptors::declared_by_render_system_pin`, lu au bout des pointeurs que
// PortCls suivra) et non à la valeur qui a servi à bâtir les deux. Son échec part au journal
// d'événements, comme celui de `check_cable_pins`.
// ---------------------------------------------------------------------------------

/// Ce qui sépare ce que la topologie d'un câble déclare de ce que sa broche wave déclare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopoMismatch {
    /// Aucun descripteur de topologie pour ce câble : les deux miniports se replieraient
    /// sur celui du câble 0 et l'endpoint porterait le nom d'un autre.
    SansFiltre,
    /// La broche système du filtre wave ne déclare rien de lisible — `check_cable_pins` l'a
    /// déjà dit, mais ce garde-fou ne s'appuie pas sur l'ordre des appels.
    SansPlage,
    /// Les nœuds servent `noeuds` canaux là où la broche wave en déclare `broche`.
    Canaux {
        /// Ce que `AudioNodes::channels` rendra.
        noeuds: ULONG,
        /// Ce que la broche système déclare.
        broche: ULONG,
    },
    /// Le masque `KSAUDIO_SPEAKER_*` du jack ne compte pas ses canaux (repli hors domaine
    /// de [`mapping_rendu`]).
    Cartographie {
        /// Le masque que `JackInfo::channel_mapping` rendra au sens rendu.
        masque: ULONG,
        /// Le nombre de canaux qu'il devrait porter.
        canaux: ULONG,
    },
}

impl fmt::Display for TopoMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SansFiltre => f.write_str(
                "aucun descripteur de filtre de topologie (repli silencieux sur celui du \
                 câble 0 : l'endpoint portera le nom d'un autre câble)",
            ),
            Self::SansPlage => f.write_str(
                "la broche système du filtre wave ne porte aucune plage audio lisible : \
                 impossible de vérifier ce que la topologie déclare",
            ),
            Self::Canaux { noeuds, broche } => write!(
                f,
                "les nœuds de volume et de sourdine serviront {noeuds} canaux là où la \
                 broche système en déclare {broche}"
            ),
            Self::Cartographie { masque, canaux } => write!(
                f,
                "la cartographie de haut-parleurs du jack ({masque:#010x}) porte {} \
                 haut-parleurs pour {canaux} canaux",
                masque.count_ones()
            ),
        }
    }
}

/// Vérifie que la topologie du câble `cable` déclarera le **même** nombre de canaux que sa
/// broche wave, et que la cartographie de haut-parleurs de son jack les compte bien.
///
/// Appelée par `adapter::start_device` pour chaque câble de la réserve, juste après
/// `descriptors::check_cable_pins` et **avant** le premier `GetDescription` : les deux
/// referment le même intervalle, l'un du côté wave, l'autre du côté topologie.
///
/// IRQL : quelconque ; ne lit que des `static` immuables et un atomique.
///
/// # Erreurs
///
/// [`TopoMismatch`], qui nomme la divergence.
pub fn check_cable_topology(cable: u32) -> Result<(), TopoMismatch> {
    if topo_render_filter(cable).is_none() || topo_capture_filter(cable).is_none() {
        return Err(TopoMismatch::SansFiltre);
    }
    let (_, broche) = declared_by_render_system_pin(cable).ok_or(TopoMismatch::SansPlage)?;
    // Ce que `AudioNodes::channels` et `CableConfig::channels` rendront, par le même chemin
    // qu'elles : le magasin des formats, et non la valeur qui a bâti la table.
    let noeuds = ULONG::from(cable_format(cable).channels);
    if noeuds != broche {
        return Err(TopoMismatch::Canaux { noeuds, broche });
    }
    // Ce que `JackInfo::channel_mapping` rendra au sens rendu : autant de haut-parleurs que
    // de canaux, sans quoi le repli stéréo de `mapping_rendu` est passé par là.
    let masque = mapping_rendu(cable_format(cable).channels);
    if masque.count_ones() != noeuds {
        return Err(TopoMismatch::Cartographie {
            masque,
            canaux: noeuds,
        });
    }
    Ok(())
}

/// Miniport topologie du filtre `TopoRender<n>`.
#[derive(Debug)]
pub struct TopoRender {
    /// Numéro du câble.
    pub n: u32,
    /// L'état partagé du câble, qui porte les nœuds du sens rendu.
    pub cable: &'static Cable,
}

impl TopoRender {
    /// Les nœuds volume et sourdine du sens rendu.
    fn nodes(&self) -> &'static NodeState {
        self.cable.nodes(Direction::Render)
    }
}

impl MiniportTopology for TopoRender {
    // IRQL: PASSIVE_LEVEL — le seul endroit d'où l'`IPortEvents` du port est atteignable
    // (voir l'en-tête de module).
    fn init(
        &self,
        _adapter: Option<ComRef<IUnknown>>,
        _resources: ResourceList,
        port: PortTopology,
    ) -> NtStatus {
        kmd_log!("TopoRender{}::Init", self.n);
        match PortEvents::from_port(port.com_ref()) {
            Ok(events) => {
                self.cable.attach_jack_events(
                    Direction::Render,
                    JackTarget::new(events, TOPO_RENDER_PIN_ENDPOINT),
                );
                STATUS_SUCCESS
            }
            Err(status) => {
                kmd_log!(
                    "TopoRender{} : QueryInterface(IID_IPortEvents) a échoué ({status:#010x}), Init échoue",
                    self.n
                );
                status
            }
        }
    }

    // IRQL: PASSIVE_LEVEL
    fn description(&self) -> &'static PCFILTER_DESCRIPTOR {
        // Les seize filtres ne diffèrent que par le GUID de nom de leur broche endpoint,
        // d'où l'indexation par `self.n`. Le trait rend une **référence**, pas une
        // `Option` : le repli sur le câble 0 n'est là que pour qu'aucun chemin ne panique
        // (même idiome que `cable::NodeState::volume`). `n < CABLE_COUNT` est garanti par
        // l'appelant : `adapter::install_cable` ne construit ce miniport que pour un
        // câble dont `cable::cable(n)` et `portcls::subdevice_names(n)` ont répondu.
        //
        // Le repli se dit, comme celui de `wave::WaveRender::description` : deux endpoints
        // portant le nom du câble 0 seraient indistinguables dans le panneau de son, et
        // rien d'autre ne le signalerait.
        match topo_render_filter(self.n) {
            Some(filtre) => filtre,
            None => {
                kmd_log!(
                    "TopoRender{} : aucun descripteur pour ce câble — repli sur celui du \
                     câble 0, l'endpoint portera le mauvais nom",
                    self.n
                );
                topo_render_filter_0()
            }
        }
    }
}

impl AudioNodes for TopoRender {
    // IRQL: PASSIVE_LEVEL — le nombre de canaux du câble (M1b-05), lu une fois au
    // démarrage : il doit s.accorder avec le format que les broches déclarent.
    fn channels(&self) -> u32 {
        u32::from(cable_format(self.n).channels)
    }

    // IRQL: PASSIVE_LEVEL
    fn volume(&self, channel: u32) -> i32 {
        self.nodes().volume(channel)
    }

    // IRQL: PASSIVE_LEVEL — mémorise, n'applique rien (voir l'en-tête de module).
    fn set_volume(&self, channel: u32, level: i32) {
        self.nodes().set_volume(channel, level);
    }

    // IRQL: PASSIVE_LEVEL
    fn muted(&self) -> bool {
        self.nodes().muted()
    }

    // IRQL: PASSIVE_LEVEL — mémorise, n'applique rien.
    fn set_muted(&self, muted: bool) {
        self.nodes().set_muted(muted);
    }

    // IRQL: PASSIVE_LEVEL
    fn trace(&self, trace: &Trace<'_>) {
        kmd_log!("TopoRender{} : {trace:?}", self.n);
    }
}

impl JackInfo for TopoRender {
    // IRQL: PASSIVE_LEVEL
    fn pin_count(&self) -> u32 {
        BROCHES
    }

    // IRQL: PASSIVE_LEVEL — la broche endpoint, pas la broche bridge (en-tête de module).
    fn jack_pin(&self) -> u32 {
        TOPO_RENDER_PIN_ENDPOINT
    }

    // IRQL: PASSIVE_LEVEL — broche de rendu analogique : cartographie non nulle, celle du
    // nombre de canaux du câble (M1b-05).
    fn channel_mapping(&self) -> u32 {
        mapping_rendu(cable_format(self.n).channels)
    }

    // IRQL: quelconque — l'état de connexion est par câble, commun aux deux sens.
    fn is_connected(&self) -> bool {
        self.cable.is_connected()
    }

    // IRQL: PASSIVE_LEVEL
    fn trace(&self, trace: &JackTrace<'_>) {
        kmd_log!("TopoRender{} : {trace:?}", self.n);
    }
}

impl CableConfig for TopoRender {
    // IRQL: PASSIVE_LEVEL
    fn cable_index(&self) -> u32 {
        self.cable.index
    }

    // IRQL: PASSIVE_LEVEL — le nombre de canaux réellement servi par ce câble ; un `SET`
    // qui en demanderait un autre est refusé (`CableState::channels_appliquables`).
    fn channels(&self) -> u32 {
        u32::from(cable_format(self.n).channels)
    }

    // IRQL: quelconque — l'état de connexion est par câble, commun aux deux sens.
    fn is_connected(&self) -> bool {
        self.cable.is_connected()
    }

    // IRQL: PASSIVE_LEVEL — applique en mémoire puis persiste ; l'`Err` ne dit que l'échec
    // de la persistance, et `portcls::config` ne le propage pas à l'appelant.
    fn set_connected(&self, connected: bool) -> Result<(), NtStatus> {
        self.cable.set_connected(connected)
    }

    // IRQL: PASSIVE_LEVEL — voir la réserve sur le contexte de fil dans `crate::privilege`.
    fn may_configure(&self) -> bool {
        privilege::may_load_driver()
    }

    // IRQL: quelconque — les compteurs sont par câble, comme l'état de connexion : les deux
    // sens d'un même câble rendent le même instantané, la boucle locale étant unique.
    fn counters(&self) -> CableCounters {
        self.cable.counters_snapshot()
    }

    // IRQL: PASSIVE_LEVEL — l'instantané porte les **deux** sens : la question du lot 0 est
    // celle du câble, et le filtre interrogé n'en choisit pas la moitié. Contrairement aux
    // compteurs, cette lecture prend les verrous (câble puis flux) : voir
    // `Cable::transport_snapshot`, qui dit pourquoi il n'y avait pas d'autre choix.
    fn transport(&self) -> CableTransport {
        self.cable.transport_snapshot()
    }

    // IRQL: PASSIVE_LEVEL — les deux sens, comme le transport. Le mode effectif vient du
    // registre et non du câble : `Cable::packets_snapshot` ne connaît pas les paramètres du
    // pilote, et c'est ici — au point où l'on répond à la propriété — qu'on sait dire dans
    // quel mode les compteurs ont été pris.
    fn packets(&self) -> CablePackets {
        CablePackets {
            packet_mode: u32::from(crate::registry::packet_mode()),
            ..self.cable.packets_snapshot()
        }
    }

    // IRQL: PASSIVE_LEVEL
    fn trace(&self, trace: &ConfigTrace<'_>) {
        kmd_log!("TopoRender{} : {trace:?}", self.n);
    }
}

impl EventSource for TopoRender {
    // IRQL: PASSIVE_LEVEL (verbe `ADD`) — la copie prend une référence de plus, sous le
    // verrou des destinataires : elle reste utilisable même si ce miniport se démonte.
    fn port_events(&self) -> Option<PortEvents> {
        self.cable.jack_port_events(Direction::Render)
    }

    // IRQL: PASSIVE_LEVEL
    fn trace(&self, trace: &EventTrace) {
        kmd_log!("TopoRender{} : {trace:?}", self.n);
    }
}

impl Drop for TopoRender {
    /// Retire le destinataire d'événement du câble et **relâche la référence COM** prise
    /// dans `Init`.
    ///
    /// C'est le bon endroit et il n'y en a pas d'autre : ce `Drop` suit le `Release` final
    /// que PortCls fait sur ce miniport, donc tout appel entrant est déjà terminé, et il
    /// précède la disparition de l'objet, donc le câble ne gardera pas de cible morte. Un
    /// signalement concurrent est indifférent à ce retrait : il passe par le même verrou et
    /// tient déjà une copie comptée s'il a vu la cible (voir `portcls::JackTargets`).
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn drop(&mut self) {
        self.cable.detach_jack_events(Direction::Render);
    }
}

/// Miniport topologie du filtre `TopoCapture<n>`.
#[derive(Debug)]
pub struct TopoCapture {
    /// Numéro du câble.
    pub n: u32,
    /// L'état partagé du câble, qui porte les nœuds du sens capture.
    pub cable: &'static Cable,
}

impl TopoCapture {
    /// Les nœuds volume et sourdine du sens capture.
    fn nodes(&self) -> &'static NodeState {
        self.cable.nodes(Direction::Capture)
    }
}

impl MiniportTopology for TopoCapture {
    // IRQL: PASSIVE_LEVEL — voir `TopoRender::init` : même geste, l'autre sens, et **sa**
    // broche endpoint.
    fn init(
        &self,
        _adapter: Option<ComRef<IUnknown>>,
        _resources: ResourceList,
        port: PortTopology,
    ) -> NtStatus {
        kmd_log!("TopoCapture{}::Init", self.n);
        match PortEvents::from_port(port.com_ref()) {
            Ok(events) => {
                self.cable.attach_jack_events(
                    Direction::Capture,
                    JackTarget::new(events, TOPO_CAPTURE_PIN_ENDPOINT),
                );
                STATUS_SUCCESS
            }
            Err(status) => {
                kmd_log!(
                    "TopoCapture{} : QueryInterface(IID_IPortEvents) a échoué ({status:#010x}), Init échoue",
                    self.n
                );
                status
            }
        }
    }

    // IRQL: PASSIVE_LEVEL
    fn description(&self) -> &'static PCFILTER_DESCRIPTOR {
        // Repli documenté sur le câble 0, et dit : voir `TopoRender::description`.
        match topo_capture_filter(self.n) {
            Some(filtre) => filtre,
            None => {
                kmd_log!(
                    "TopoCapture{} : aucun descripteur pour ce câble — repli sur celui du \
                     câble 0, l'endpoint portera le mauvais nom",
                    self.n
                );
                topo_capture_filter_0()
            }
        }
    }
}

impl AudioNodes for TopoCapture {
    // IRQL: PASSIVE_LEVEL — le nombre de canaux du câble (M1b-05), lu une fois au
    // démarrage : il doit s.accorder avec le format que les broches déclarent.
    fn channels(&self) -> u32 {
        u32::from(cable_format(self.n).channels)
    }

    // IRQL: PASSIVE_LEVEL
    fn volume(&self, channel: u32) -> i32 {
        self.nodes().volume(channel)
    }

    // IRQL: PASSIVE_LEVEL — mémorise, n'applique rien (voir l'en-tête de module).
    fn set_volume(&self, channel: u32, level: i32) {
        self.nodes().set_volume(channel, level);
    }

    // IRQL: PASSIVE_LEVEL
    fn muted(&self) -> bool {
        self.nodes().muted()
    }

    // IRQL: PASSIVE_LEVEL — mémorise, n'applique rien.
    fn set_muted(&self, muted: bool) {
        self.nodes().set_muted(muted);
    }

    // IRQL: PASSIVE_LEVEL
    fn trace(&self, trace: &Trace<'_>) {
        kmd_log!("TopoCapture{} : {trace:?}", self.n);
    }
}

impl JackInfo for TopoCapture {
    // IRQL: PASSIVE_LEVEL
    fn pin_count(&self) -> u32 {
        BROCHES
    }

    // IRQL: PASSIVE_LEVEL — l'endpoint est ici la broche d'**entrée** (en-tête de module).
    fn jack_pin(&self) -> u32 {
        TOPO_CAPTURE_PIN_ENDPOINT
    }

    // IRQL: PASSIVE_LEVEL — broche de capture : cartographie **nulle**, la documentation
    // l'exige.
    fn channel_mapping(&self) -> u32 {
        MAPPING_CAPTURE
    }

    // IRQL: quelconque — le même atomique que le sens rendu, un câble ayant deux bouts.
    fn is_connected(&self) -> bool {
        self.cable.is_connected()
    }

    // IRQL: PASSIVE_LEVEL
    fn trace(&self, trace: &JackTrace<'_>) {
        kmd_log!("TopoCapture{} : {trace:?}", self.n);
    }
}

impl CableConfig for TopoCapture {
    // IRQL: PASSIVE_LEVEL
    fn cable_index(&self) -> u32 {
        self.cable.index
    }

    // IRQL: PASSIVE_LEVEL — le nombre de canaux réellement servi par ce câble ; un `SET`
    // qui en demanderait un autre est refusé (`CableState::channels_appliquables`).
    fn channels(&self) -> u32 {
        u32::from(cable_format(self.n).channels)
    }

    // IRQL: quelconque — le même atomique que le sens rendu, un câble ayant deux bouts.
    fn is_connected(&self) -> bool {
        self.cable.is_connected()
    }

    // IRQL: PASSIVE_LEVEL — voir `TopoRender` : même câble, même effet des deux côtés.
    fn set_connected(&self, connected: bool) -> Result<(), NtStatus> {
        self.cable.set_connected(connected)
    }

    // IRQL: PASSIVE_LEVEL — voir la réserve sur le contexte de fil dans `crate::privilege`.
    fn may_configure(&self) -> bool {
        privilege::may_load_driver()
    }

    // IRQL: quelconque — voir `TopoRender` : la boucle locale est unique par câble, les
    // deux sens rendent donc le même instantané.
    fn counters(&self) -> CableCounters {
        self.cable.counters_snapshot()
    }

    // IRQL: PASSIVE_LEVEL — voir `TopoRender` : les deux sens, quel que soit le filtre visé.
    fn transport(&self) -> CableTransport {
        self.cable.transport_snapshot()
    }

    // IRQL: PASSIVE_LEVEL — voir `TopoRender`, y compris pour le mode effectif.
    fn packets(&self) -> CablePackets {
        CablePackets {
            packet_mode: u32::from(crate::registry::packet_mode()),
            ..self.cable.packets_snapshot()
        }
    }

    // IRQL: PASSIVE_LEVEL
    fn trace(&self, trace: &ConfigTrace<'_>) {
        kmd_log!("TopoCapture{} : {trace:?}", self.n);
    }
}

impl EventSource for TopoCapture {
    // IRQL: PASSIVE_LEVEL — voir `TopoRender`, l'autre sens du même câble.
    fn port_events(&self) -> Option<PortEvents> {
        self.cable.jack_port_events(Direction::Capture)
    }

    // IRQL: PASSIVE_LEVEL
    fn trace(&self, trace: &EventTrace) {
        kmd_log!("TopoCapture{} : {trace:?}", self.n);
    }
}

impl Drop for TopoCapture {
    /// Retire le destinataire d'événement du câble et relâche la référence COM prise dans
    /// `Init` : voir `TopoRender`, même raisonnement, l'autre sens.
    ///
    /// IRQL : `PASSIVE_LEVEL`.
    fn drop(&mut self) {
        self.cable.detach_jack_events(Direction::Capture);
    }
}
