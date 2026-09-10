//! Numérotation des paquets WaveRT : trames absolues ↔ numéro de paquet `ULONG`
//! (lot 3 du mode paquets).
//!
//! # Un paquet est une période, et son numéro tient sur 32 bits
//!
//! Le mode paquets ne nomme rien de neuf : « The packet size is the WaveRT buffer size
//! divided by the `NotificationCount` passed to `AllocateBufferWithNotification` »
//! (`GetReadPacket`, Remarks). Le numéro d'un paquet est donc l'indice de la période qui
//! le contient — exactement ce que `crate::notify::Notifier` calcule déjà pour décider
//! quand signaler —, et son décalage dans le tampon cyclique est
//! `(numéro % NotificationCount) × taille_de_paquet`.
//!
//! Deux domaines se rencontrent ici, et c'est toute la raison d'être de ce module :
//!
//! - le pilote raisonne sur une position **absolue en trames** (`u64`, monotone, jamais
//!   réduite modulo le tampon : voir [`crate::position`]), d'où un numéro de paquet
//!   absolu lui aussi `u64` ;
//! - les quatre méthodes de `portcls.h` échangent un `ULONG`, c'est-à-dire un numéro
//!   **tronqué à 32 bits**, qui reboucle. La documentation de `SetWritePacket` le dit sans
//!   détour : la comparaison de deux numéros est **modulaire**.
//!
//! [`resolve`] est le seul pont entre les deux, et il ne suppose rien du client : il rend
//! le numéro absolu congruent à ce qui est annoncé, **le plus proche** d'une ancre que le
//! pilote choisit dans son propre état. Un client qui recommence sa numérotation, ou qui
//! reboucle après 2³² paquets (vingt-quatre jours à 10 ms de période), se recale par
//! `GetPacketCount` — c'est la fonction que la documentation désigne pour cela, et rien
//! d'autre ne le permettrait : un événement de notification ne dit pas quel paquet il
//! concerne.
//!
//! # Ce que ce module décide, et ce qu'il ne décide pas
//!
//! Il décide **l'arithmétique** : quel numéro absolu correspond à un `ULONG` annoncé,
//! quel paquet est complet à une position donnée, et si une annonce est en retard
//! ([`WriteVerdict::Late`]) ou trop en avance ([`WriteVerdict::Overrun`]).
//!
//! Il ne décide **rien du transport** : ni le tampon, ni les verrous, ni le minuteur.
//! `SetWritePacket` reste un *hint* qui n'exempte pas le pilote « d'incrémenter son
//! compteur interne de paquets et de signaler les événements de notification à une cadence
//! temps réel nominale » ; ce module ne connaît même pas l'existence d'un minuteur.
//!
//! Tout ici est `const` ou pur, sans allocation, sans panique et sans opérateur
//! arithmétique nu : appelable à `DISPATCH_LEVEL` comme le reste du crate — même si les
//! quatre méthodes, elles, arrivent à `PASSIVE_LEVEL`.

/// Moitié du domaine des numéros de paquet 32 bits (2³¹).
///
/// C'est la frontière de [`resolve`] : un écart modulaire inférieur désigne un paquet
/// **devant** l'ancre, un écart supérieur ou égal un paquet **derrière**. Écrite en
/// constante parce que `u32::MAX / 2` serait une division, refusée par
/// `clippy::arithmetic_side_effects`, pour une valeur qui ne changera jamais.
const HALF: u32 = 0x8000_0000;

/// Géométrie des paquets d'un flux : la taille d'un paquet et le nombre de paquets que le
/// tampon cyclique contient.
///
/// Les deux viennent d'`AllocateBufferWithNotification` : la taille est
/// `tampon / NotificationCount` (soit [`crate::notify::Notifier::period_frames`]) et le
/// nombre est ce `NotificationCount`. Un flux dont le tampon a été alloué **sans**
/// notifications n'a pas de géométrie de paquets du tout — [`PacketGeometry::new`] rend
/// alors `None`, et c'est la façon dont le pilote refuse les quatre méthodes sans avoir à
/// se demander pourquoi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketGeometry {
    /// Trames par paquet (≥ 1).
    packet_frames: u64,
    /// Paquets par tour de tampon (≥ 1) : le `NotificationCount` demandé.
    packets_per_buffer: u64,
}

impl PacketGeometry {
    /// Géométrie d'un tampon de `packets_per_buffer` paquets de `packet_frames` trames.
    ///
    /// `None` si l'une des deux valeurs est nulle : il n'y a alors pas de paquet à
    /// numéroter, et toute la suite serait une division par zéro déguisée.
    pub const fn new(packet_frames: u64, packets_per_buffer: u64) -> Option<Self> {
        if packet_frames == 0 || packets_per_buffer == 0 {
            return None;
        }
        Some(Self {
            packet_frames,
            packets_per_buffer,
        })
    }

    /// Trames par paquet.
    pub const fn packet_frames(&self) -> u64 {
        self.packet_frames
    }

    /// Paquets par tour de tampon (`NotificationCount`).
    pub const fn packets_per_buffer(&self) -> u64 {
        self.packets_per_buffer
    }

    /// Numéro **absolu** du paquet qui contient la trame absolue `frames`.
    ///
    /// C'est `Notifier::period_of` sous un autre nom : le mode paquets ne calcule rien de
    /// plus que ce que la boucle de notification calculait déjà.
    pub const fn packet_of(&self, frames: u64) -> u64 {
        // `packet_frames ≥ 1` par construction : jamais `None`.
        match frames.checked_div(self.packet_frames) {
            Some(paquet) => paquet,
            None => 0,
        }
    }

    /// Première trame absolue du paquet `packet` (saturée à `u64::MAX`).
    pub const fn first_frame_of(&self, packet: u64) -> u64 {
        packet.saturating_mul(self.packet_frames)
    }

    /// Trame absolue qui **suit** la dernière du paquet `packet`.
    ///
    /// C'est la valeur que le pilote retient comme borne de validation d'un
    /// `SetWritePacket` : le client qui annonce le paquet *n* a écrit les trames
    /// `[n × taille, (n+1) × taille)`, et rien au-delà.
    pub const fn end_frame_of(&self, packet: u64) -> u64 {
        self.first_frame_of(packet)
            .saturating_add(self.packet_frames)
    }

    /// Nombre de paquets **entièrement** écoulés à la position `frames`, en base 1.
    ///
    /// « If the packet count is 5, then 5 packets have completely transferred. That is,
    /// packets 0-4 have completely transferred. » (`GetPacketCount`, Remarks.) C'est
    /// exactement `packet_of`, et les deux noms coexistent parce que les deux questions ne
    /// se posent pas dans le même sens : l'une désigne un paquet, l'autre en compte.
    pub const fn complete_at(&self, frames: u64) -> u64 {
        self.packet_of(frames)
    }

    /// Décalage du paquet `packet` dans le tampon cyclique, en trames.
    ///
    /// `(numéro % NotificationCount) × taille_de_paquet` : la formule de `GetReadPacket`,
    /// écrite une seule fois. Le pilote ne s'en sert pas pour copier — `crate::ring` fait
    /// sa propre réduction cyclique sur les trames absolues — mais c'est l'adresse que le
    /// moteur audio calculera de son côté, et la vérifier ici est ce qui garantit qu'on
    /// parle du même octet.
    pub const fn ring_offset_frames(&self, packet: u64) -> u64 {
        // `packets_per_buffer ≥ 1` par construction : jamais `None`.
        let dans_le_tour = match packet.checked_rem(self.packets_per_buffer) {
            Some(reste) => reste,
            None => 0,
        };
        dans_le_tour.saturating_mul(self.packet_frames)
    }
}

/// Numéro **absolu** congruent à `announced` modulo 2³², le plus proche de `anchor`.
///
/// Le client n'envoie que 32 bits ; le pilote raisonne sur 64. La règle est celle qu'on
/// applique à tout compteur qui reboucle : parmi tous les entiers congruents à `announced`,
/// retenir celui qui tombe dans la demi-fenêtre autour de l'ancre — 2³¹ paquets devant,
/// 2³¹ derrière. Un écart plus grand n'est pas représentable et ne se distinguerait pas
/// d'un rebouclage ; c'est la limite du contrat, pas de cette fonction.
///
/// Saturée aux deux bouts : un numéro « derrière » plus grand que l'ancre rend 0 plutôt
/// que de déborder, ce qui laisse la validation le refuser proprement.
pub const fn resolve(anchor: u64, announced: u32) -> u64 {
    let bas = (anchor & 0xFFFF_FFFF) as u32;
    // Distance modulaire **vers l'avant** de l'ancre au numéro annoncé.
    let avant = announced.wrapping_sub(bas);
    if avant < HALF {
        anchor.saturating_add(avant as u64)
    } else {
        // Au-delà de la moitié du domaine, l'annonce est derrière : la distance vers
        // l'arrière est l'écart modulaire dans l'autre sens, calculé sans soustraction nue.
        anchor.saturating_sub(bas.wrapping_sub(announced) as u64)
    }
}

/// Le numéro `ULONG` d'un numéro de paquet absolu : ses 32 bits de poids faible.
///
/// C'est ce que le pilote écrit dans `*PacketNumber` de `GetReadPacket`, et ce que
/// [`resolve`] sait relire.
pub const fn truncate(packet: u64) -> u32 {
    (packet & 0xFFFF_FFFF) as u32
}

/// Ce que le pilote répond à un `SetWritePacket`.
///
/// Les deux refus sont les codes que `portcls.h` documente, et pas d'autres : le moteur
/// audio les traite (il se recale par `GetPacketCount`), là où un `STATUS_UNSUCCESSFUL`
/// ne lui apprendrait rien.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteVerdict {
    /// Annonce retenue : le numéro **absolu** du paquet que le client vient d'écrire.
    Accepted(u64),
    /// `STATUS_DATA_LATE_ERROR` : le paquet est déjà transféré, ou son transfert a
    /// commencé — le client est arrivé après la position de lecture. C'est aussi la
    /// réponse à un numéro qui **recule** sous le dernier accepté.
    Late,
    /// `STATUS_DATA_OVERRUN` : le paquet est trop en avance pour tenir dans le tampon —
    /// l'écrire écraserait des trames que le pilote n'a pas encore lues.
    Overrun,
}

/// Valide l'annonce `announced` d'un `SetWritePacket` sur un flux de rendu.
///
/// - `geo` : la géométrie du tampon du flux ;
/// - `position_frames` : la position absolue du flux à l'instant de l'appel
///   ([`crate::position::StreamPosition::frames_at`]) ;
/// - `last_written` : le dernier numéro **absolu** accepté, `None` avant le premier.
///
/// L'ancre de [`resolve`] est le paquet attendu — celui qui suit le dernier accepté, ou
/// celui que la position désigne au premier appel. C'est ce qui fait qu'un client qui
/// reprend sa numérotation à zéro après un `KSSTATE_STOP` ne se voit pas projeté deux
/// milliards de paquets en arrière : au `STOP`, le pilote oublie `last_written` **et** la
/// position, donc l'ancre repart de zéro elle aussi.
///
/// # Les trois refus, et lequel s'applique en premier
///
/// Un numéro qui recule est **en retard**, jamais un débordement : il désigne des trames
/// que le pilote a déjà lues, pas des trames qu'il n'a pas encore lues. L'ordre des tests
/// suit donc cette lecture — retard d'abord, débordement ensuite — et il est observable :
/// un numéro très en arrière et un numéro très en avant ne doivent pas rendre le même
/// code, sans quoi le client ne saurait pas dans quel sens se recaler.
pub fn validate_write(
    geo: &PacketGeometry,
    position_frames: u64,
    last_written: Option<u64>,
    announced: u32,
) -> WriteVerdict {
    let ancre = match last_written {
        Some(dernier) => dernier.saturating_add(1),
        None => geo.packet_of(position_frames),
    };
    let absolu = resolve(ancre, announced);
    // Déjà accepté (ou plus ancien) : le client réécrit un paquet qu'il nous a déjà donné.
    if matches!(last_written, Some(dernier) if absolu <= dernier) {
        return WriteVerdict::Late;
    }
    // Le transfert du paquet a commencé, ou il est fini : la position est entrée dedans.
    if geo.first_frame_of(absolu) < position_frames {
        return WriteVerdict::Late;
    }
    // Trop en avance pour tenir : le tampon ne contient que `packets_per_buffer` paquets à
    // partir de celui qui joue.
    let courant = geo.packet_of(position_frames);
    if absolu >= courant.saturating_add(geo.packets_per_buffer()) {
        return WriteVerdict::Overrun;
    }
    WriteVerdict::Accepted(absolu)
}

/// Ce que le pilote répond à un `GetReadPacket`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadVerdict {
    /// Un paquet complet est disponible, et il n'a pas encore été rendu.
    Ready {
        /// Numéro **absolu** du paquet.
        packet: u64,
        /// Vrai si des paquets complets ont été **sautés** depuis le dernier rendu : le
        /// pilote les a écrasés avant que le moteur ne vienne les chercher. C'est
        /// l'information de trou, que le pilote reporte en drapeau
        /// `KSSTREAM_HEADER_OPTIONSF_DATADISCONTINUITY`.
        gap: bool,
    },
    /// Aucun paquet neuf : `STATUS_DEVICE_NOT_READY`. Jamais un `Ok` sur un paquet déjà
    /// rendu — c'est la seule chose que la documentation interdise explicitement.
    NotReady,
}

/// Choisit le paquet que `GetReadPacket` doit rendre sur un flux de capture.
///
/// - `position_frames` : la position absolue du flux, c'est-à-dire jusqu'où la boucle
///   locale a écrit ;
/// - `last_read` : le dernier numéro **absolu** rendu, `None` avant le premier.
///
/// Le paquet rendu est le **dernier complet**, pas le plus ancien non lu : « When the OS
/// calls this routine, the driver may assume that the OS has finished reading all previous
/// packets. » Rendre un paquet ancien alors qu'un plus récent existe ferait donc perdre au
/// moteur audio tout ce qui les sépare, sans qu'il le sache. Le saut, lui, se **dit** —
/// [`ReadVerdict::Ready::gap`] —, ce qui est le seul comportement honnête possible : les
/// trames manquantes ont été écrasées, on ne peut que le signaler.
///
/// Ne bloque jamais, ne prend rien, n'attend rien : c'est un *pull*, et le réveil du
/// client reste l'événement de `RegisterNotificationEvent`.
pub fn next_read(
    geo: &PacketGeometry,
    position_frames: u64,
    last_read: Option<u64>,
) -> ReadVerdict {
    let complets = geo.complete_at(position_frames);
    // Aucun paquet entier écrit : le flux vient de démarrer.
    let Some(candidat) = complets.checked_sub(1) else {
        return ReadVerdict::NotReady;
    };
    match last_read {
        Some(dernier) if candidat <= dernier => ReadVerdict::NotReady,
        Some(dernier) => ReadVerdict::Ready {
            packet: candidat,
            gap: candidat > dernier.saturating_add(1),
        },
        None => ReadVerdict::Ready {
            packet: candidat,
            gap: false,
        },
    }
}

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
    use proptest::prelude::*;

    /// 10 ms à 48 kHz, deux paquets par tampon : la géométrie du mode partagé.
    fn geo() -> PacketGeometry {
        PacketGeometry::new(480, 2).unwrap()
    }

    #[test]
    fn une_geometrie_veut_deux_valeurs_non_nulles() {
        assert!(PacketGeometry::new(0, 2).is_none());
        assert!(PacketGeometry::new(480, 0).is_none());
        let g = geo();
        assert_eq!(g.packet_frames(), 480);
        assert_eq!(g.packets_per_buffer(), 2);
    }

    #[test]
    fn le_numero_de_paquet_est_l_indice_de_periode() {
        let g = geo();
        assert_eq!(g.packet_of(0), 0);
        assert_eq!(g.packet_of(479), 0);
        assert_eq!(g.packet_of(480), 1);
        assert_eq!(g.packet_of(959), 1);
        assert_eq!(g.packet_of(960), 2);
        assert_eq!(g.first_frame_of(3), 1_440);
        assert_eq!(g.end_frame_of(3), 1_920);
        // Base 1 : à 960 trames, deux paquets (0 et 1) sont complets.
        assert_eq!(g.complete_at(0), 0);
        assert_eq!(g.complete_at(479), 0);
        assert_eq!(g.complete_at(960), 2);
    }

    #[test]
    fn le_decalage_cyclique_suit_la_formule_de_getreadpacket() {
        let g = geo();
        assert_eq!(g.ring_offset_frames(0), 0);
        assert_eq!(g.ring_offset_frames(1), 480);
        // Deux paquets par tampon : le paquet 2 retombe au début.
        assert_eq!(g.ring_offset_frames(2), 0);
        assert_eq!(g.ring_offset_frames(3), 480);
        let quatre = PacketGeometry::new(240, 4).unwrap();
        assert_eq!(quatre.ring_offset_frames(6), 480);
    }

    #[test]
    fn resolve_choisit_le_congruent_le_plus_proche() {
        // Sans rebouclage : l'annonce est le numéro lui-même.
        assert_eq!(resolve(100, 100), 100);
        assert_eq!(resolve(100, 103), 103);
        assert_eq!(resolve(100, 97), 97);
        // Rebouclage de l'ancre : 2³² + 3 annoncé « 3 » alors que l'ancre est 2³² + 1.
        let ancre = (1u64 << 32) + 1;
        assert_eq!(resolve(ancre, 3), (1u64 << 32) + 3);
        assert_eq!(resolve(ancre, 0), 1u64 << 32);
        // Une annonce juste sous 2³² depuis une ancre juste au-dessus : c'est le paquet
        // d'avant le rebouclage, pas quatre milliards de paquets en avant.
        assert_eq!(resolve(ancre, u32::MAX), (1u64 << 32) - 1);
        // Saturation au plancher : rien ne descend sous zéro.
        assert_eq!(resolve(5, u32::MAX), 0);
        // Saturation au plafond.
        assert_eq!(resolve(u64::MAX, 0), u64::MAX);
    }

    #[test]
    fn truncate_et_resolve_sont_reciproques() {
        for absolu in [0u64, 1, 479, 1 << 31, (1 << 32) - 1, 1 << 32, (1 << 40) + 7] {
            assert_eq!(resolve(absolu, truncate(absolu)), absolu, "{absolu}");
        }
    }

    /// **La table du contrat de `SetWritePacket`**, ligne par ligne : ce que le client
    /// annonce, où en est la position, et le code qu'il reçoit.
    #[test]
    fn setwritepacket_table() {
        struct Cas {
            nom: &'static str,
            position: u64,
            dernier: Option<u64>,
            annonce: u32,
            attendu: WriteVerdict,
        }
        let g = geo();
        let cas = [
            Cas {
                nom: "premier paquet d'un flux qui démarre",
                position: 0,
                dernier: None,
                annonce: 0,
                attendu: WriteVerdict::Accepted(0),
            },
            Cas {
                nom: "le paquet suivant, pendant que le premier joue",
                position: 10,
                dernier: Some(0),
                annonce: 1,
                attendu: WriteVerdict::Accepted(1),
            },
            Cas {
                nom: "un numéro déjà accepté : en retard",
                position: 10,
                dernier: Some(1),
                annonce: 1,
                attendu: WriteVerdict::Late,
            },
            Cas {
                nom: "un numéro qui recule : en retard, jamais un débordement",
                position: 4_800,
                dernier: Some(10),
                annonce: 4,
                attendu: WriteVerdict::Late,
            },
            Cas {
                nom: "un paquet dont le transfert a commencé : en retard",
                position: 500,
                dernier: None,
                annonce: 1,
                attendu: WriteVerdict::Late,
            },
            Cas {
                nom: "deux paquets par tampon : le troisième déborde",
                position: 0,
                dernier: Some(1),
                annonce: 2,
                attendu: WriteVerdict::Overrun,
            },
            Cas {
                nom: "un saut qui tient encore dans le tampon est accepté",
                position: 0,
                dernier: None,
                annonce: 1,
                attendu: WriteVerdict::Accepted(1),
            },
            Cas {
                nom: "un saut très en avant déborde",
                position: 0,
                dernier: None,
                annonce: 1_000,
                attendu: WriteVerdict::Overrun,
            },
        ];
        for c in cas {
            assert_eq!(
                validate_write(&g, c.position, c.dernier, c.annonce),
                c.attendu,
                "{}",
                c.nom
            );
        }
    }

    /// Un cycle de rendu complet : paquets 0, 1, 2… acceptés l'un après l'autre pendant
    /// que la position avance, chacun bornant la copie à sa fin.
    #[test]
    fn un_cycle_de_rendu_accepte_les_paquets_dans_l_ordre() {
        let g = geo();
        let mut dernier = None;
        let mut position = 0u64;
        for n in 0u32..64 {
            let verdict = validate_write(&g, position, dernier, n);
            let WriteVerdict::Accepted(absolu) = verdict else {
                panic!("paquet {n} refusé : {verdict:?}");
            };
            assert_eq!(absolu, u64::from(n));
            // Ce que le paquet valide : les trames jusqu'à sa fin, et pas une de plus.
            assert_eq!(g.end_frame_of(absolu), u64::from(n + 1) * 480);
            dernier = Some(absolu);
            // Le client écrit un paquet d'avance : la position suit avec un paquet de
            // retard, ce qui garde `absolu` dans la fenêtre du tampon.
            position = absolu.saturating_mul(480);
        }
    }

    #[test]
    fn getreadpacket_rend_le_dernier_complet_et_dit_les_trous() {
        let g = geo();
        // Rien de complet avant la fin du premier paquet.
        assert_eq!(next_read(&g, 0, None), ReadVerdict::NotReady);
        assert_eq!(next_read(&g, 479, None), ReadVerdict::NotReady);
        // Le paquet 0 est complet à 480 trames.
        assert_eq!(
            next_read(&g, 480, None),
            ReadVerdict::Ready {
                packet: 0,
                gap: false
            }
        );
        // Déjà rendu : rien de neuf, et surtout pas un second `Ok` sur le même paquet.
        assert_eq!(next_read(&g, 480, Some(0)), ReadVerdict::NotReady);
        assert_eq!(next_read(&g, 959, Some(0)), ReadVerdict::NotReady);
        // Le suivant, sans trou.
        assert_eq!(
            next_read(&g, 960, Some(0)),
            ReadVerdict::Ready {
                packet: 1,
                gap: false
            }
        );
        // Un moteur qui ne revient qu'après trois paquets : le dernier complet, et le trou
        // annoncé.
        assert_eq!(
            next_read(&g, 2_400, Some(0)),
            ReadVerdict::Ready {
                packet: 4,
                gap: true
            }
        );
    }

    proptest! {
        /// `resolve` rend toujours un congruent, et l'écart à l'ancre reste dans la
        /// demi-fenêtre — sauf saturation aux deux bouts, où il ne peut pas.
        #[test]
        fn resolve_rend_un_congruent_proche(
            ancre in any::<u64>(),
            annonce in any::<u32>(),
        ) {
            let absolu = resolve(ancre, annonce);
            // Saturé : le seul cas où la congruence peut être perdue, et il se reconnaît.
            let sature = absolu == 0 || absolu == u64::MAX;
            if !sature {
                prop_assert_eq!(truncate(absolu), annonce, "congruence perdue");
                let ecart = absolu.abs_diff(ancre);
                prop_assert!(ecart <= u64::from(HALF), "écart {ecart} hors demi-fenêtre");
            }
        }

        /// Une annonce quelconque ne panique jamais, et un verdict accepté est un numéro
        /// congruent qui tient dans le tampon **et** n'est pas déjà transféré.
        #[test]
        fn validate_write_ne_promet_que_du_tenable(
            packet_frames in 1u64..=48_000,
            packets in 1u64..=8,
            position in any::<u64>(),
            dernier in proptest::option::of(any::<u64>()),
            annonce in any::<u32>(),
        ) {
            let g = PacketGeometry::new(packet_frames, packets).unwrap();
            match validate_write(&g, position, dernier, annonce) {
                WriteVerdict::Accepted(absolu) => {
                    prop_assert_eq!(truncate(absolu), annonce, "numéro non congruent accepté");
                    // Pas déjà transféré : le paquet commence à la position ou après.
                    prop_assert!(g.first_frame_of(absolu) >= position);
                    // Pas déjà accepté.
                    if let Some(d) = dernier {
                        prop_assert!(absolu > d);
                    }
                    // Tient dans le tampon.
                    prop_assert!(absolu < g.packet_of(position).saturating_add(packets));
                }
                WriteVerdict::Late | WriteVerdict::Overrun => {}
            }
        }

        /// `next_read` est **monotone** : rejoué sur des positions croissantes, il ne rend
        /// jamais deux fois le même paquet ni un paquet plus ancien, et le paquet rendu est
        /// toujours complet à la position.
        #[test]
        fn next_read_est_monotone(
            packet_frames in 1u64..=48_000,
            packets in 1u64..=8,
            pas in proptest::collection::vec(0u64..100_000, 1..80),
        ) {
            let g = PacketGeometry::new(packet_frames, packets).unwrap();
            let mut position = 0u64;
            let mut dernier = None::<u64>;
            for p in pas {
                position = position.saturating_add(p);
                match next_read(&g, position, dernier) {
                    ReadVerdict::Ready { packet, gap } => {
                        if let Some(d) = dernier {
                            prop_assert!(packet > d, "un paquet rendu deux fois");
                            prop_assert_eq!(gap, packet > d + 1);
                        } else {
                            prop_assert!(!gap, "un trou avant le premier paquet");
                        }
                        // Le paquet rendu est entièrement écrit.
                        prop_assert!(g.end_frame_of(packet) <= position);
                        dernier = Some(packet);
                    }
                    ReadVerdict::NotReady => {}
                }
            }
        }

        /// Le décalage cyclique reste dans le tampon, et il est un multiple de la taille de
        /// paquet : c'est l'adresse que le moteur audio calculera de son côté.
        #[test]
        fn le_decalage_cyclique_reste_dans_le_tampon(
            packet_frames in 1u64..=48_000,
            packets in 1u64..=8,
            paquet in any::<u64>(),
        ) {
            let g = PacketGeometry::new(packet_frames, packets).unwrap();
            let offset = g.ring_offset_frames(paquet);
            prop_assert!(offset < packet_frames.saturating_mul(packets));
            prop_assert_eq!(offset % packet_frames, 0);
        }
    }
}
