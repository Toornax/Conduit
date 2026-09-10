//! Les **contraintes de taille de paquet** que le pilote déclare au moteur audio
//! (`KSAUDIO_PACKETSIZE_CONSTRAINTS2`, `DEVPKEY_KsAudio_PacketSize_Constraints2`).
//!
//! # Le fait mesuré qui rend ce module nécessaire
//!
//! `IAudioClient3::GetSharedModeEnginePeriod` annonce, sur nos endpoints : défaut 480
//! trames, fondamentale 480, **minimum 480, maximum 480**. Autrement dit 10 ms, et rien
//! d'autre — aucune application ne peut demander une période plus courte, quoi qu'elle
//! fasse. Ce n'est pas une limite du moteur : « Low Latency Audio » range la déclaration
//! de `DEVPKEY_KsAudio_PacketSize_Constraints2` parmi les **obligations** du pilote, et
//! nous ne la déclarions nulle part. Sans elle, Windows retombe sur son défaut historique
//! de 10 ms.
//!
//! Ce module ne pose rien — il ne connaît ni le noyau ni PnP : il **calcule les valeurs**
//! que `conduit_kmd::adapter` posera, à partir de ce que le pilote sait réellement faire.
//! C'est la partie qui se teste sans machine.
//!
//! # Les valeurs, et d'où elles viennent
//!
//! | Champ | Valeur | Ce qui la fixe |
//! |---|---|---|
//! | `MinPacketPeriodInHns` | [`MIN_PACKET_PERIOD_HNS`] | le minuteur de 1 ms du pilote, et rien d'autre |
//! | `PacketSizeFileAlignment` | [`PACKET_SIZE_FILE_ALIGNMENT`] | rien : le pilote n'impose aucun alignement en octets |
//! | `MaxPacketSizeInBytes` | [`MAX_PACKET_SIZE_BYTES`] | 10 ms du plus gros format qu'une broche puisse servir |
//! | `NumProcessingModeConstraints` | [`PROCESSING_MODE_CONSTRAINT_COUNT`] | la **longueur** du tableau ci-dessous, jamais un littéral |
//! | `ProcessingModeConstraints[0]` | [`AUDIO_SIGNALPROCESSINGMODE_DEFAULT`], en durée | [`processing_packet_duration_hns`] : le plancher `BufferMs`, tenu **au-dessus** de la période minimale |
//!
//! # Deux valeurs, parce que la documentation en exige deux différentes
//!
//! Ces deux champs répondent à deux questions distinctes, et la seconde doit **dépasser**
//! la première :
//!
//! - `MinPacketPeriodInHns` demande à quelle cadence le pilote sait rendre un paquet. La
//!   réponse est son **minuteur** : la boucle locale bat à 1 ms, donc 2 ms, quelle que soit
//!   la taille de tampon réglée dans le registre ([`MIN_PACKET_PERIOD_HNS`]).
//! - `ProcessingPacketDurationInHns` demande la taille de paquet du mode de traitement. La
//!   réponse est le plancher `BufferMs`, divisé par [`NOTIFICATION_COUNT`], parce qu'un
//!   paquet WaveRT n'est pas le tampon : « Several WaveRT packets (typically 2) are
//!   concatenated to form the WaveRT buffer » (documentation de
//!   `KSAUDIO_PACKETSIZE_CONSTRAINTS2`), et la mesure le montre — un client WASAPI exclusif
//!   événementiel obtient `AllocateBufferWithNotification` avec **2 × 240 trames**.
//!
//! Les confondre coûterait la contrainte tout entière : « Low Latency Audio » prévient que
//! « the mode-specific constraints need to be **higher** than the drivers minimum buffer
//! size, otherwise they're ignored by the audio stack ». Une durée **égale** à la période
//! minimale n'est pas supérieure : elle est ignorée. D'où
//! [`PROCESSING_DURATION_MARGIN_HNS`], qui tient l'inégalité stricte même pour un `BufferMs`
//! si court que sa moitié passerait sous les 2 ms.
//!
//! La conséquence — un client peut demander un tampon plus court que le plancher — est
//! traitée sur [`PacketConstraints`] : le pilote alloue plus grand et le **dit**, ce que
//! WaveRT permet.

use crate::config::ConfigGuid;
use crate::format::{MAX_BUFFER_MS, MIN_BUFFER_MS, SAMPLE_DEPTHS, SAMPLE_RATES};
use crate::ring::FrameLayout;

/// Unités de cent nanosecondes dans une milliseconde.
pub const HNS_PER_MS: u32 = 10_000;

/// Nombre de paquets concaténés qu'on suppose dans un tampon WaveRT.
///
/// Deux : c'est le « typically 2 » de la documentation, et c'est ce que la mesure a rendu
/// (`AllocateBufferWithNotification`, 2 × 240 trames, client WASAPI exclusif
/// événementiel). C'est le diviseur de [`processing_packet_duration_hns`] : le tampon vaut
/// deux paquets, donc le paquet vaut la moitié du plancher `BufferMs`.
pub const NOTIFICATION_COUNT: u32 = 2;

/// `MinPacketPeriodInHns` : **20 000 hns**, soit 2 ms. Une constante.
///
/// C'est la période de transport la plus courte que le pilote sache honorer, et elle ne
/// dépend que de son **minuteur** : la boucle locale bat à 1 ms (`conduit_kmd::timer`), donc
/// une période de 1 ms se retrouverait à la merci d'un seul tick en retard, et un tick perdu
/// sur deux périodes s'entend. Deux millisecondes laissent un tick de marge, et c'est aussi
/// ce que déclare l'exemple `SysvadWaveRtPacketSizeConstraintsRender` de la documentation
/// Microsoft.
///
/// `BufferMs` n'entre **pas** dans ce champ, alors qu'il y entrait jusqu'au lot précédent :
/// une période minimale qui suivait le plancher de tampon montait avec la durée de la
/// contrainte de mode, si bien que les deux restaient éternellement égales — et une
/// contrainte de mode qui n'est pas *supérieure* à la période minimale est ignorée par la
/// pile audio. Ce que `BufferMs` gouverne, c'est [`processing_packet_duration_hns`].
pub const MIN_PACKET_PERIOD_HNS: u32 = 2u32.saturating_mul(HNS_PER_MS);

/// La marge qui garde la durée de la contrainte de mode **strictement** au-dessus de
/// [`MIN_PACKET_PERIOD_HNS`] : 1 ms, soit un tick de minuteur.
///
/// « Low Latency Audio » : « the mode-specific constraints need to be **higher** than the
/// drivers minimum buffer size, otherwise they're ignored by the audio stack ». *Higher*,
/// donc : l'égalité ne compte pas, et une contrainte ignorée ne laisse aucune trace — le
/// seul symptôme serait la période de 10 ms qu'on essaie précisément de faire descendre.
///
/// Pour le défaut `BufferMs = 10`, la moitié du plancher (5 ms) dépasse déjà largement les
/// 2 ms et cette marge ne sert à rien. Elle tient l'inégalité pour les tampons **courts** :
/// tout `BufferMs` inférieur à 6 ms donnerait sinon une durée sous la période minimale, ou
/// juste dessus.
pub const PROCESSING_DURATION_MARGIN_HNS: u32 = HNS_PER_MS;

/// `PacketSizeFileAlignment` : **`FILE_BYTE_ALIGNMENT`**, c'est-à-dire aucune contrainte.
///
/// # Pourquoi zéro, et pas une taille de page
///
/// Le champ est un **masque** d'alignement (`wdm.h` : `FILE_BYTE_ALIGNMENT` = 0,
/// `FILE_WORD_ALIGNMENT` = 1, … `FILE_512_BYTE_ALIGNMENT` = 0x1ff), pas une taille : la
/// taille d'un paquet doit être un multiple de `masque + 1`. La documentation de
/// `KSAUDIO_PACKETSIZE_CONSTRAINTS2` énumère les dix valeurs admises, et la plus grande
/// vaut 512 octets ; 4096 n'en fait pas partie.
///
/// Surtout, un alignement de 4096 octets **interdirait** ce que ce module cherche à
/// obtenir. À 48 kHz sur deux canaux en flottant (8 octets par trame), le plus petit
/// paquet multiple de 4096 octets ferait 512 trames, soit 10,67 ms : plus long que la
/// période de 10 ms qu'on essaie précisément de faire descendre. Déclarer une contrainte
/// que le pilote n'a pas coûterait donc exactement la latence qu'on vient chercher.
///
/// Le pilote, lui, n'exige rien de tel : sa copie cyclique travaille en **trames**
/// (`conduit_kmd_core::ring`), et le moteur audio calcule déjà ses paquets en trames. Zéro
/// est donc la valeur vraie, et c'est celle que l'exemple Microsoft déclare lui aussi
/// (`FILE_BYTE_ALIGNMENT`, « 1 byte packet size alignment »).
pub const PACKET_SIZE_FILE_ALIGNMENT: u32 = 0;

/// Durée, en millisecondes, du paquet le plus long que le pilote annonce pouvoir servir.
///
/// Dix, parce que la documentation l'exige : « This size should at least be large enough
/// to support 10 ms buffer of any format supported by the pin ».
pub const MAX_PACKET_MS: u32 = 10;

/// La plus haute fréquence d'échantillonnage qu'une broche puisse servir, en hertz.
///
/// Lue dans [`SAMPLE_RATES`] plutôt que recopiée : une quatrième fréquence ajoutée un jour
/// agrandirait [`MAX_PACKET_SIZE_BYTES`] toute seule, au lieu de laisser une constante
/// mentir.
const fn max_sample_rate() -> u32 {
    // Motif de tranche plutôt qu'indexation : le crate interdit `a[i]`.
    let mut restantes: &[u32] = &SAMPLE_RATES;
    let mut max = 0;
    while let [premiere, reste @ ..] = restantes {
        if *premiere > max {
            max = *premiere;
        }
        restantes = reste;
    }
    max
}

/// Le plus gros échantillon qu'une broche puisse servir, en octets.
///
/// Lu dans [`SAMPLE_DEPTHS`], pour la raison de [`max_sample_rate`].
const fn max_sample_bytes() -> u32 {
    let mut restantes: &[crate::ring::SampleFormat] = &SAMPLE_DEPTHS;
    let mut max = 0;
    while let [premiere, reste @ ..] = restantes {
        let octets = premiere.bytes_per_sample();
        if octets > max {
            max = octets;
        }
        restantes = reste;
    }
    max
}

/// La plus grosse trame qu'une broche puisse servir, en octets : le maximum de canaux fois
/// le plus gros échantillon (8 × 4 = 32 octets).
pub const MAX_FRAME_BYTES: u32 =
    (FrameLayout::MAX_CHANNELS as u32).saturating_mul(max_sample_bytes());

/// `MaxPacketSizeInBytes` : [`MAX_PACKET_MS`] du plus gros format qu'une broche puisse
/// servir, arrondi à [`PACKET_SIZE_FILE_ALIGNMENT`].
///
/// 96 000 Hz × 8 canaux × 4 octets × 10 ms = **30 720 octets**. La valeur est un
/// **maximum**, pas une promesse d'allocation : le pilote sait allouer jusqu'à
/// [`MAX_BUFFER_MS`] (500 ms) et n'aura donc jamais à refuser un paquet qu'il a annoncé.
///
/// Elle ne dépend **pas** du format réglé sur le câble. La contrainte se pose sur
/// l'interface du filtre, une fois, et la documentation parle de « any format supported by
/// the pin » : la borne doit couvrir la plus large des variantes, pas celle du jour.
pub const MAX_PACKET_SIZE_BYTES: u32 = arrondi_alignement(
    max_sample_rate()
        .saturating_mul(MAX_FRAME_BYTES)
        .saturating_mul(MAX_PACKET_MS)
        .saturating_div(1_000),
    PACKET_SIZE_FILE_ALIGNMENT,
);

/// Arrondit `octets` **vers le haut** au multiple de `masque + 1`.
///
/// `masque` est un masque d'alignement au sens de `wdm.h` (`FILE_*_ALIGNMENT`), donc
/// toujours de la forme `2^k − 1` ; zéro n'arrondit rien. La saturation de l'addition ne
/// peut arrondir vers le bas que sur une valeur déjà proche de `u32::MAX`, que ce module
/// ne produit jamais (le plus gros paquet fait quelques dizaines de kilo-octets).
const fn arrondi_alignement(octets: u32, masque: u32) -> u32 {
    octets.saturating_add(masque) & !masque
}

/// `ProcessingPacketDurationInHns` de l'unique contrainte de mode, pour un plancher de
/// tampon de `buffer_ms` millisecondes.
///
/// Deux bornes, et la plus grande gagne :
///
/// - **le plancher de tampon** : `conduit_kmd::stream` remonte tout tampon à `BufferMs`
///   (`buffer_bytes_with_floor`), et un tampon vaut [`NOTIFICATION_COUNT`] paquets ; la
///   taille de paquet que le moteur audio obtiendra réellement est donc `BufferMs / 2` ;
/// - **l'inégalité stricte** : jamais moins de [`MIN_PACKET_PERIOD_HNS`] +
///   [`PROCESSING_DURATION_MARGIN_HNS`], une contrainte de mode qui ne **dépasse** pas la
///   période minimale annoncée étant ignorée par la pile audio.
///
/// `buffer_ms` est écrêté dans `MIN_BUFFER_MS..=MAX_BUFFER_MS`, comme le fait
/// [`crate::params::sanitize`] : la valeur vient du registre et un appelant distrait ne
/// doit pas pouvoir faire annoncer une durée aberrante.
///
/// Avec le défaut `BufferMs = 10` : `max(10 × 10 000 / 2, 20 000 + 10 000)` = **50 000
/// hns**, soit 5 ms — la moitié des 10 ms d'aujourd'hui, et deux fois et demie la période
/// minimale de 2 ms.
#[must_use]
pub const fn processing_packet_duration_hns(buffer_ms: u32) -> u32 {
    let buffer_ms = if buffer_ms < MIN_BUFFER_MS {
        MIN_BUFFER_MS
    } else if buffer_ms > MAX_BUFFER_MS {
        MAX_BUFFER_MS
    } else {
        buffer_ms
    };
    let demi_plancher = buffer_ms
        .saturating_mul(HNS_PER_MS)
        .saturating_div(NOTIFICATION_COUNT);
    let strictement_au_dessus =
        MIN_PACKET_PERIOD_HNS.saturating_add(PROCESSING_DURATION_MARGIN_HNS);
    if demi_plancher > strictement_au_dessus {
        demi_plancher
    } else {
        strictement_au_dessus
    }
}

/// `AUDIO_SIGNALPROCESSINGMODE_DEFAULT` (`ksmedia.h` du WDK 26100, l. 8424-8426,
/// `{C18E2F7E-933D-4965-B7D1-1EEF228D2AF3}`) : le **mode de traitement par défaut**, celui
/// que le moteur audio applique à tout flux qui n'en demande pas d'autre.
///
/// C'est le mode de l'unique contrainte que [`PacketConstraints`] déclare, et le seul qui
/// ait un sens ici : le pilote n'expose ni `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES` ni le
/// moindre attribut de mode sur ses plages de format, donc tout ce qu'il sert passe par
/// `DEFAULT`. Poser la contrainte sur un autre mode — `RAW`, `MOVIE`, `COMMUNICATIONS` — la
/// rendrait vraie pour un mode que personne n'emprunte : une panne **muette**, dont le seul
/// symptôme serait la période de 10 ms qu'on essaie précisément de faire descendre.
///
/// # La chaîne de confiance, la même que pour `KSPROPSETID_CONDUIT`
///
/// La valeur est **recopiée** ici plutôt qu'importée, ce crate étant portable et sans
/// dépendance (voir [`ConfigGuid`]) ; elle est donc adossée à deux oracles indépendants.
/// `portcls::adapter` vérifie par assertion `const` que sa conversion en `GUID` est
/// **identique** au `portcls_sys::AUDIO_SIGNALPROCESSINGMODE_DEFAULT` que bindgen extrait
/// du `DEFINE_GUIDSTRUCT` de `ksmedia.h`, et `portcls-sys/tests/layout.rs` confronte ce
/// dernier à ce que `cl.exe` lit de la macro `STATIC_AUDIO_SIGNALPROCESSINGMODE_DEFAULT`
/// du même en-tête (`layout.golden`). Un chiffre faux ici tombe donc à la compilation du
/// pilote, jamais en machine.
pub const AUDIO_SIGNALPROCESSINGMODE_DEFAULT: ConfigGuid = ConfigGuid {
    data1: 0xC18E_2F7E,
    data2: 0x933D,
    data3: 0x4965,
    data4: [0xB7, 0xD1, 0x1E, 0xEF, 0x22, 0x8D, 0x2A, 0xF3],
};

/// Nombre de contraintes de mode de traitement que le pilote déclare : **une**, celle de
/// [`AUDIO_SIGNALPROCESSINGMODE_DEFAULT`].
///
/// C'est la longueur du tableau [`PacketConstraints::processing_modes`], et c'est **elle**
/// que `portcls::adapter` recopie dans `NumProcessingModeConstraints` : un littéral écrit
/// à la main des deux côtés finirait par diverger du tableau, et le moteur audio lirait
/// alors une entrée que le pilote n'a pas remplie — ou n'en lirait pas une qu'il a remplie.
/// Le `ANYSIZE_ARRAY` du WDK vaut lui aussi 1, si bien que la conversion tient dans la
/// structure C sans allocation de queue.
pub const PROCESSING_MODE_CONSTRAINT_COUNT: usize = 1;

/// Une entrée du tableau `ProcessingModeConstraints` :
/// `KSAUDIO_PACKETSIZE_PROCESSINGMODE_CONSTRAINT` du WDK, champ pour champ.
///
/// Structure **portable**, comme [`PacketConstraints`] : elle ne mentionne aucun type du
/// WDK et se teste en mode utilisateur. Elle est néanmoins `#[repr(C)]` et ses décalages
/// sont vérifiés contre ceux que `cl.exe` a mesurés (`layout.golden`, lignes
/// `KSAUDIO_PACKETSIZE_PROCESSINGMODE_CONSTRAINT.*` : 24 octets, `ProcessingMode` en 0,
/// `SamplesPerProcessingPacket` en 16, `ProcessingPacketDurationInHns` en 20) — non parce
/// que ce crate la sérialise, mais parce qu'une disposition qui divergerait de celle du WDK
/// rendrait le miroir trompeur.
///
/// # Les deux derniers champs ne sont pas cumulatifs : le premier **prime**
///
/// La documentation est explicite, et c'est la seule chose à retenir de cette structure :
/// `SamplesPerProcessingPacket` est « the processing frame size for the processing mode,
/// expressed in number of samples. **If this value is 0, the constraint is expressed by the
/// `ProcessingPacketDurationInHns` field** », et `ProcessingPacketDurationInHns` la même
/// taille « expressed in hundred-nanosecond (HNS) units. **This field is ignored if
/// `SamplesPerProcessingPacket` is nonzero** ».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)]
pub struct ProcessingModeConstraint {
    /// `ProcessingMode: GUID` : le mode de traitement que cette entrée contraint.
    pub mode: ConfigGuid,
    /// `SamplesPerProcessingPacket: ULONG` : la taille en **échantillons**, ou zéro pour
    /// exprimer la contrainte en durée (voir la note de structure).
    pub samples_per_processing_packet: u32,
    /// `ProcessingPacketDurationInHns: ULONG` : la taille en **centaines de nanosecondes**,
    /// ignorée si le champ précédent est non nul.
    pub processing_packet_duration_hns: u32,
}

// La disposition est bien celle du WDK : ce sont exactement les nombres que `cl.exe` a
// mesurés dans `drivers\windows\portcls-sys\tests\layout.golden`.
const _: () = assert!(size_of::<ProcessingModeConstraint>() == 24);
const _: () = assert!(align_of::<ProcessingModeConstraint>() == 4);
const _: () = assert!(core::mem::offset_of!(ProcessingModeConstraint, mode) == 0);
const _: () =
    assert!(core::mem::offset_of!(ProcessingModeConstraint, samples_per_processing_packet) == 16);
const _: () =
    assert!(core::mem::offset_of!(ProcessingModeConstraint, processing_packet_duration_hns) == 20);

/// Les valeurs de `KSAUDIO_PACKETSIZE_CONSTRAINTS2`, dans l'ordre de la structure du WDK.
///
/// Une structure **portable** : elle ne mentionne aucun type du WDK, ce qui la rend
/// calculable et testable en mode utilisateur. `portcls::adapter` la recopie dans la vraie
/// `KSAUDIO_PACKETSIZE_CONSTRAINTS2`, dont la disposition est vérifiée contre `cl.exe`
/// (`layout.golden`).
///
/// # L'hypothèse que la contrainte de mode teste
///
/// Les trois premières valeurs sont déclarées depuis le lot précédent, et **le moteur audio
/// n'a pas bougé** : `IAudioClient3::GetSharedModeEnginePeriod` annonce toujours 480 trames
/// de défaut, de fondamentale, de minimum et de maximum, alors que la `DEVPKEY` est bien
/// posée — vérifiée en machine virtuelle, `STATUS_SUCCESS` relu par
/// `KSPROPERTY_CONDUIT_TRANSPORT`. Reste une différence avec l'exemple de la documentation
/// Microsoft : `SysvadWaveRtPacketSizeConstraintsRender` porte, lui, une entrée de mode de
/// traitement, là où nous n'en déclarions aucune. C'est donc **cette seule variable** qu'on
/// bouge — une à la fois, comme toujours : une entrée pour
/// [`AUDIO_SIGNALPROCESSINGMODE_DEFAULT`], et rien d'autre (ni attribut de mode sur les
/// plages de format, ni `KSPROPERTY_AUDIOSIGNALPROCESSING_MODES`, ni mode `RAW`).
///
/// L'entrée est exprimée **en durée** : `samples_per_processing_packet` vaut zéro — « if
/// this value is 0, the constraint is expressed by the `ProcessingPacketDurationInHns`
/// field » — et `processing_packet_duration_hns` porte
/// [`processing_packet_duration_hns()`](processing_packet_duration_hns). Une contrainte en
/// échantillons aurait dû choisir une fréquence, or le pilote sert 44,1, 48 **et** 96 kHz
/// sur la même interface de filtre : 240 échantillons y valent 5 ms à 48 kHz et 2,5 ms à
/// 96 kHz, c'est-à-dire deux contraintes différentes selon le format réglé. La durée, elle,
/// est vraie pour les trois.
///
/// # L'inégalité stricte, et ce qu'elle laisse au pilote à faire
///
/// « Low Latency Audio » prévient : « the mode-specific constraints need to be **higher**
/// than the drivers minimum buffer size, otherwise they're ignored by the audio stack ».
/// D'où deux valeurs qui ne se suivent plus : [`MIN_PACKET_PERIOD_HNS`] dit ce que le
/// **transport** sait faire (2 ms, la cadence du minuteur, constante) et
/// [`processing_packet_duration_hns`] ce que le plancher `BufferMs` impose (5 ms pour le
/// défaut), la seconde restant strictement au-dessus de la première pour **tout**
/// `buffer_ms` admissible.
///
/// Le pilote annonce donc une période de transport plus courte que la taille de tampon
/// qu'il alloue vraiment, et c'est déjà son comportement d'aujourd'hui. Un client qui lit
/// les 2 ms peut demander deux paquets de 2 ms, soit un tampon de 4 ms, sous le plancher
/// `BufferMs` : `conduit_kmd::stream::allocate_inner` le remonte alors à `BufferMs`
/// ([`crate::format::buffer_bytes_with_floor`]) et **rend la taille réellement allouée** —
/// un client WaveRT lit la taille de son tampon, il ne la dicte pas. La période effective
/// vaut alors `taille réelle / NotificationCount`, c'est-à-dire les 5 ms de
/// [`processing_packet_duration_hns`] et non les 2 ms demandées. Le pilote ne refuse jamais
/// un paquet qu'il a annoncé : il sert plus long que demandé, en le disant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PacketConstraints {
    /// `MinPacketPeriodInHns` : la période la plus courte que le pilote accepte de servir.
    pub min_packet_period_hns: u32,
    /// `PacketSizeFileAlignment` : le masque d'alignement en octets d'un paquet.
    pub packet_size_file_alignment: u32,
    /// `MaxPacketSizeInBytes` : le plus gros paquet que le pilote accepte de servir.
    pub max_packet_size_bytes: u32,
    /// `ProcessingModeConstraints` : les contraintes par mode de traitement, une seule ici.
    ///
    /// `NumProcessingModeConstraints` n'a **pas** de champ : c'est la longueur de ce
    /// tableau ([`PROCESSING_MODE_CONSTRAINT_COUNT`]), et l'écrire deux fois serait offrir
    /// à la structure et à son compte l'occasion de diverger.
    pub processing_modes: [ProcessingModeConstraint; PROCESSING_MODE_CONSTRAINT_COUNT],
}

impl PacketConstraints {
    /// Les contraintes du pilote pour un plancher de tampon de `buffer_ms` millisecondes.
    ///
    /// La période minimale est la constante [`MIN_PACKET_PERIOD_HNS`] — le transport ne
    /// dépend pas du registre — tandis que l'unique contrainte de mode suit `buffer_ms`, en
    /// durée, par [`processing_packet_duration_hns`]. La seconde **dépasse** toujours la
    /// première, comme la documentation l'exige : c'est ce que fixe le test
    /// `la_contrainte_de_mode_depasse_toujours_la_periode_minimale`, sur toute la plage
    /// admissible de `buffer_ms`.
    #[must_use]
    pub const fn new(buffer_ms: u32) -> Self {
        Self {
            min_packet_period_hns: MIN_PACKET_PERIOD_HNS,
            packet_size_file_alignment: PACKET_SIZE_FILE_ALIGNMENT,
            max_packet_size_bytes: MAX_PACKET_SIZE_BYTES,
            processing_modes: [ProcessingModeConstraint {
                mode: AUDIO_SIGNALPROCESSINGMODE_DEFAULT,
                samples_per_processing_packet: 0,
                processing_packet_duration_hns: processing_packet_duration_hns(buffer_ms),
            }; PROCESSING_MODE_CONSTRAINT_COUNT],
        }
    }

    /// La période minimale annoncée, en **trames** à `sample_rate` hertz.
    ///
    /// Ce que `IAudioClient3::GetSharedModeEnginePeriod` finira par rendre en
    /// `minPeriodInFrames`, et donc la seule forme de cette valeur qui se compare aux 480
    /// trames mesurées. Arrondie **vers le haut** : une période un peu plus longue reste
    /// servable, une période un peu plus courte ne l'est pas.
    ///
    /// `None` si `sample_rate` est nul (aucune trame ne dure alors quoi que ce soit).
    #[must_use]
    pub const fn min_period_frames(&self, sample_rate: u32) -> Option<u32> {
        if sample_rate == 0 {
            return None;
        }
        // trames = ceil(période_hns × fréquence / 10 000 000), en 64 bits : à 96 kHz et
        // 500 ms, le produit dépasse largement 32 bits.
        let hns = self.min_packet_period_hns as u64;
        let par_seconde = HNS_PER_MS.saturating_mul(1_000) as u64;
        let numerateur = match hns.checked_mul(sample_rate as u64) {
            Some(n) => n,
            None => return None,
        };
        let arrondi = numerateur.saturating_add(par_seconde.saturating_sub(1));
        // `checked_div` plutôt que `saturating_div` : le diviseur est une constante non
        // nulle, mais le lint anti-panique du crate ne le sait pas, et une division dont le
        // diviseur ne se lit pas sur place mérite d'être écrite faillible.
        let Some(trames) = arrondi.checked_div(par_seconde) else {
            return None;
        };
        if trames > u32::MAX as u64 {
            None
        } else {
            Some(trames as u32)
        }
    }
}

// Les valeurs constantes du module sont ce que le pilote déclarera : une assertion à la
// compilation vaut mieux qu'un test qu'on oublierait de lancer.
const _: () = assert!(MAX_FRAME_BYTES == 32, "8 canaux × 4 octets");
const _: () = assert!(
    MAX_PACKET_SIZE_BYTES == 30_720,
    "96 kHz × 32 octets × 10 ms"
);
const _: () = assert!(
    MIN_PACKET_PERIOD_HNS == 20_000,
    "2 ms, le minuteur de 1 ms doublé"
);
// L'inégalité que « Low Latency Audio » exige, tenue dès le plus petit tampon admissible :
// en dessous, la contrainte de mode serait ignorée sans le moindre message.
const _: () = assert!(processing_packet_duration_hns(MIN_BUFFER_MS) > MIN_PACKET_PERIOD_HNS);
const _: () = assert!(processing_packet_duration_hns(10) == 50_000, "5 ms");
// Le maximum annoncé couvre bien dix millisecondes du plus gros format, sans quoi la
// contrainte serait plus étroite que ce que la documentation exige.
const _: () = assert!(MAX_PACKET_SIZE_BYTES >= 96_000u32.saturating_mul(32).saturating_div(100));

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::panic
)]
mod tests {
    use super::*;

    #[test]
    fn le_defaut_annonce_deux_millisecondes_de_periode() {
        let contraintes = PacketConstraints::new(10);
        assert_eq!(contraintes.min_packet_period_hns, 20_000, "2 ms");
        assert_eq!(contraintes.packet_size_file_alignment, 0);
        assert_eq!(contraintes.max_packet_size_bytes, 30_720);
    }

    /// La période minimale est celle du **minuteur**, pas celle du registre : elle ne bouge
    /// pour aucun `buffer_ms`, y compris les deux valeurs aberrantes que l'écrêtage rattrape.
    #[test]
    fn la_periode_minimale_ne_depend_pas_du_plancher_de_tampon() {
        for buffer_ms in [0, 1, 4, 5, 6, 10, 20, 50, 100, 500, u32::MAX] {
            assert_eq!(
                PacketConstraints::new(buffer_ms).min_packet_period_hns,
                MIN_PACKET_PERIOD_HNS,
                "tampon {buffer_ms} ms"
            );
        }
    }

    /// L'unique contrainte de mode vise `DEFAULT`, s'exprime **en durée** (donc
    /// `samples = 0`) et porte les cinq millisecondes du défaut.
    #[test]
    fn le_defaut_porte_une_contrainte_de_mode_par_defaut_en_duree() {
        let contraintes = PacketConstraints::new(10);
        assert_eq!(contraintes.processing_modes.len(), 1);
        assert_eq!(
            contraintes.processing_modes.len(),
            PROCESSING_MODE_CONSTRAINT_COUNT,
            "le compte annoncé est la longueur du tableau"
        );
        let entree = contraintes.processing_modes[0];
        assert_eq!(entree.mode, AUDIO_SIGNALPROCESSINGMODE_DEFAULT);
        // Zéro : « if this value is 0, the constraint is expressed by the
        // ProcessingPacketDurationInHns field ». Non nul, il primerait sur la durée.
        assert_eq!(entree.samples_per_processing_packet, 0);
        assert_eq!(entree.processing_packet_duration_hns, 50_000, "5 ms");
    }

    /// **L'invariant de ce module** : pour tout `BufferMs` que le registre puisse rendre
    /// après [`crate::params::sanitize`], la durée de la contrainte de mode est
    /// *strictement* supérieure à la période minimale annoncée.
    ///
    /// « The mode-specific constraints need to be **higher** than the drivers minimum buffer
    /// size, otherwise they're ignored by the audio stack » : l'égalité ne compte pas, et une
    /// contrainte ignorée ne dit rien — elle laisse la période à 10 ms sans un message. Les
    /// bornes sont celles de `params.rs`, qui sont par construction celles de `format.rs`
    /// (`params::tests::bornes_coherentes_avec_le_voisin`).
    #[test]
    fn la_contrainte_de_mode_depasse_toujours_la_periode_minimale() {
        for buffer_ms in crate::params::MIN_BUFFER_MS..=crate::params::MAX_BUFFER_MS {
            let contraintes = PacketConstraints::new(buffer_ms);
            let entree = contraintes.processing_modes[0];
            assert_eq!(
                entree.samples_per_processing_packet, 0,
                "tampon {buffer_ms} ms"
            );
            assert_eq!(
                entree.processing_packet_duration_hns,
                processing_packet_duration_hns(buffer_ms),
                "tampon {buffer_ms} ms"
            );
            assert!(
                entree.processing_packet_duration_hns > contraintes.min_packet_period_hns,
                "tampon {buffer_ms} ms : {} devrait dépasser {}",
                entree.processing_packet_duration_hns,
                contraintes.min_packet_period_hns
            );
        }
    }

    /// Le même invariant aux deux valeurs qu'aucun `BufferMs` légal ne produit, mais qu'un
    /// appelant distrait pourrait passer : l'écrêtage les rattrape, l'inégalité tient.
    #[test]
    fn l_inegalite_tient_aussi_pour_les_durees_aberrantes() {
        for buffer_ms in [0, u32::MAX] {
            let contraintes = PacketConstraints::new(buffer_ms);
            assert!(
                contraintes.processing_modes[0].processing_packet_duration_hns
                    > contraintes.min_packet_period_hns,
                "tampon {buffer_ms} ms"
            );
        }
    }

    /// `AUDIO_SIGNALPROCESSINGMODE_DEFAULT` vaut bien
    /// `{C18E2F7E-933D-4965-B7D1-1EEF228D2AF3}`, écrit en toutes lettres et champ par
    /// champ : un chiffre de travers poserait la contrainte sur un mode que personne
    /// n'emprunte, sans le moindre message d'erreur.
    #[test]
    fn le_mode_par_defaut_est_celui_de_ksmedia() {
        let guid = AUDIO_SIGNALPROCESSINGMODE_DEFAULT;
        assert_eq!(guid.data1, 0xC18E_2F7E);
        assert_eq!(guid.data2, 0x933D);
        assert_eq!(guid.data3, 0x4965);
        assert_eq!(guid.data4, [0xB7, 0xD1, 0x1E, 0xEF, 0x22, 0x8D, 0x2A, 0xF3]);
    }

    /// Sous 6 ms de tampon, la moitié du plancher passerait sous la période minimale (ou
    /// juste dessus) : c'est la marge qui reprend la main, à 3 ms.
    #[test]
    fn la_marge_tient_la_duree_au_dessus_des_tampons_courts() {
        let plancher = MIN_PACKET_PERIOD_HNS + PROCESSING_DURATION_MARGIN_HNS;
        assert_eq!(plancher, 30_000, "2 ms + 1 ms");
        // 1 ms de tampon voudrait 0,5 ms de paquet ; 4 ms en voudrait 2, soit exactement la
        // période minimale — ignorée. 5 ms en voudrait 2,5, encore trop peu.
        for buffer_ms in [1, 2, 3, 4, 5] {
            assert_eq!(
                processing_packet_duration_hns(buffer_ms),
                plancher,
                "tampon {buffer_ms} ms"
            );
        }
        // 6 ms : 3 ms de paquet, soit exactement la marge — la première valeur où les deux
        // bornes coïncident.
        assert_eq!(processing_packet_duration_hns(6), plancher);
        // 7 ms : 3,5 ms, le plancher de tampon prend le dessus.
        assert_eq!(processing_packet_duration_hns(7), 35_000);
    }

    #[test]
    fn la_duree_annoncee_est_la_moitie_du_plancher_de_tampon() {
        for buffer_ms in [7, 10, 20, 50, 100, 500] {
            assert_eq!(
                processing_packet_duration_hns(buffer_ms),
                buffer_ms * HNS_PER_MS / NOTIFICATION_COUNT,
                "tampon {buffer_ms} ms"
            );
        }
    }

    #[test]
    fn la_duree_du_registre_est_ecretee_comme_dans_params() {
        assert_eq!(
            processing_packet_duration_hns(0),
            processing_packet_duration_hns(MIN_BUFFER_MS)
        );
        assert_eq!(
            processing_packet_duration_hns(u32::MAX),
            processing_packet_duration_hns(MAX_BUFFER_MS)
        );
    }

    #[test]
    fn la_duree_ne_recule_jamais_quand_le_plancher_monte() {
        let mut precedente = 0;
        for buffer_ms in MIN_BUFFER_MS..=MAX_BUFFER_MS {
            let duree = processing_packet_duration_hns(buffer_ms);
            assert!(duree >= precedente, "tampon {buffer_ms} ms");
            precedente = duree;
        }
    }

    #[test]
    fn le_paquet_maximal_couvre_dix_millisecondes_de_chaque_variante() {
        for rate in SAMPLE_RATES {
            for depth in SAMPLE_DEPTHS {
                let trame = u32::from(FrameLayout::MAX_CHANNELS) * depth.bytes_per_sample();
                let dix_ms = rate * trame / 100;
                assert!(
                    MAX_PACKET_SIZE_BYTES >= dix_ms,
                    "{rate} Hz, {depth:?} : {dix_ms} octets"
                );
            }
        }
    }

    #[test]
    fn la_periode_se_lit_en_trames_comme_le_moteur_audio_la_rendra() {
        let contraintes = PacketConstraints::new(10);
        // 2 ms à 48 kHz : 96 trames, un cinquième des 480 mesurées.
        assert_eq!(contraintes.min_period_frames(48_000), Some(96));
        assert_eq!(contraintes.min_period_frames(96_000), Some(192));
        // 44,1 kHz : 88,2 trames, arrondies vers le haut.
        assert_eq!(contraintes.min_period_frames(44_100), Some(89));
        assert_eq!(contraintes.min_period_frames(0), None);
    }

    #[test]
    fn arrondir_a_un_masque_nul_ne_change_rien() {
        for octets in [0, 1, 7, 4095, 30_720] {
            assert_eq!(arrondi_alignement(octets, 0), octets);
        }
        // Un masque de 512 octets arrondit bien vers le haut.
        assert_eq!(arrondi_alignement(1, 0x1ff), 512);
        assert_eq!(arrondi_alignement(512, 0x1ff), 512);
        assert_eq!(arrondi_alignement(513, 0x1ff), 1024);
    }
}
