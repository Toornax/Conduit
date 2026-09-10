//! Cible cargo-fuzz : la numérotation des paquets WaveRT (`packetnum`, lot 3).
//!
//! Les quatre méthodes du mode paquets reçoivent, dans le pilote, un `ULONG` que le
//! **moteur audio** choisit entièrement, et le pilote le compare à un état 64 bits qu'il
//! tient lui-même. Toute l'arithmétique de cette rencontre est dans
//! `conduit_kmd_core::packetnum`, et elle tourne dans le chemin du flux : une panique y
//! serait un `KeBugCheckEx`, un débordement silencieux y ferait accepter un paquet qui
//! écrase des trames que le pilote n'a pas encore lues.
//!
//! Ce que la cible éprouve, au-delà de l'absence de panique :
//!
//! - **la congruence** : un numéro accepté par `validate_write` est bien congru modulo
//!   2³² au `ULONG` annoncé — sans quoi le pilote et le moteur ne parleraient pas du même
//!   paquet, et l'écart ne se verrait qu'après vingt-quatre jours de lecture ;
//! - **les trois promesses d'une acceptation** : le paquet n'est pas déjà transféré, il
//!   n'a pas déjà été accepté, et il tient dans le tampon. Ce sont exactement les trois
//!   refus documentés, pris à l'envers ;
//! - **la monotonie de la lecture** : `next_read` rejoué sur des positions croissantes ne
//!   rend jamais deux fois le même paquet, ni un paquet plus ancien, ni un paquet
//!   incomplet — « never return `Ok` on a packet already returned » est la seule chose que
//!   la documentation de `GetReadPacket` interdise explicitement ;
//! - **l'aller-retour** de `resolve` et `truncate`, qui est ce qui autorise le pilote à ne
//!   publier que 32 bits d'un compteur qu'il tient sur 64.
//!
//! # Pourquoi une cible à part, et pas une ligne de plus dans `etat-cable`
//!
//! `etat-cable` éprouve des **parseurs** : des octets venus de l'espace utilisateur, dont
//! la seule question est « acceptés ou refusés, et l'aller-retour est-il fidèle ». Ici il
//! n'y a pas de parseur : il y a une machine à états dont l'entrée est une **séquence**
//! (une position qui avance, des annonces successives) et dont l'invariant porte sur
//! l'histoire, pas sur un tampon. Les mêmes octets ne veulent pas dire la même chose, et
//! les mêlanger dans une seule cible diluerait les deux corpus.
//!
//! `cargo +nightly fuzz run numeros-de-paquet` depuis `crates/conduit-kmd-core`.
#![no_main]

use conduit_kmd_core::packetnum::{
    next_read, resolve, truncate, PacketGeometry, ReadVerdict, WriteVerdict,
};
use libfuzzer_sys::fuzz_target;

/// Lecteur d'octets sans dépendance : la cible construit sa séquence depuis le tampon brut
/// plutôt que par `arbitrary`, comme les trois autres cibles du crate.
struct Flux<'a> {
    reste: &'a [u8],
}

impl<'a> Flux<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { reste: data }
    }

    /// Le prochain octet, ou `None` quand l'entrée est épuisée.
    fn octet(&mut self) -> Option<u8> {
        let (premier, suite) = self.reste.split_first()?;
        self.reste = suite;
        Some(*premier)
    }

    /// Les quatre prochains octets en `u32` (petit-boutiste), ou `None`.
    fn mot(&mut self) -> Option<u32> {
        let (tete, suite) = self.reste.split_at_checked(4)?;
        self.reste = suite;
        let octets: [u8; 4] = tete.try_into().ok()?;
        Some(u32::from_le_bytes(octets))
    }
}

fuzz_target!(|data: &[u8]| {
    let mut flux = Flux::new(data);

    // La géométrie : une taille de paquet et un nombre de paquets par tampon, tous deux
    // non nuls. Les bornes sont larges à dessein — le pilote n'accepte que 1 ou 2 paquets
    // par tampon aujourd'hui, mais l'arithmétique ne doit pas en dépendre.
    let Some(taille) = flux.mot() else { return };
    let Some(paquets) = flux.octet() else { return };
    let taille = u64::from(taille.max(1));
    let paquets = u64::from(paquets.max(1));
    let Some(geo) = PacketGeometry::new(taille, paquets) else {
        panic!("une géométrie à deux valeurs non nulles doit exister");
    };

    // La position de départ, sur 32 bits : de quoi couvrir plusieurs tours de tampon sans
    // que la seule chose éprouvée soit la saturation à `u64::MAX`.
    let Some(depart) = flux.mot() else { return };
    let mut position = u64::from(depart);

    let mut dernier_ecrit: Option<u64> = None;
    let mut dernier_lu: Option<u64> = None;
    let mut derniere_position_lue = position;

    // Chaque tour : la position avance d'un pas choisi par le fuzzer, puis le client
    // annonce un numéro d'écriture et le moteur vient chercher un paquet de lecture.
    while let (Some(pas), Some(annonce)) = (flux.mot(), flux.mot()) {
        position = position.saturating_add(u64::from(pas));

        match validate(&geo, position, dernier_ecrit, annonce) {
            Some(absolu) => dernier_ecrit = Some(absolu),
            None => {}
        }

        // La lecture se fait sur une position qui ne recule jamais : c'est le contrat de
        // l'horloge du pilote (`StreamPosition::frames_at` est monotone).
        assert!(position >= derniere_position_lue);
        derniere_position_lue = position;
        if let ReadVerdict::Ready { packet, gap } = next_read(&geo, position, dernier_lu) {
            // Le paquet rendu est entièrement écrit à cette position.
            assert!(
                geo.end_frame_of(packet) <= position,
                "paquet {packet} incomplet à {position}"
            );
            match dernier_lu {
                Some(precedent) => {
                    assert!(packet > precedent, "paquet {packet} rendu deux fois");
                    assert_eq!(gap, packet > precedent.saturating_add(1), "trou mal dit");
                }
                None => assert!(!gap, "un trou avant le premier paquet"),
            }
            // Le décalage annoncé au moteur audio reste dans le tampon.
            let offset = geo.ring_offset_frames(packet);
            assert!(offset < taille.saturating_mul(paquets), "décalage hors tampon");
            dernier_lu = Some(packet);
        }
    }
});

/// Le verdict d'une annonce d'écriture, et **tout** ce qu'une acceptation promet.
///
/// Rend le numéro absolu retenu, ou `None` si l'annonce a été refusée. Les deux refus se
/// valent ici : ce qui compte est qu'un refus n'ait rien changé à l'état du pilote, et
/// qu'une acceptation tienne ses trois promesses.
fn validate(
    geo: &PacketGeometry,
    position: u64,
    dernier: Option<u64>,
    annonce: u32,
) -> Option<u64> {
    match conduit_kmd_core::packetnum::validate_write(geo, position, dernier, annonce) {
        WriteVerdict::Accepted(absolu) => {
            // 1. Le pilote et le moteur parlent bien du même paquet.
            assert_eq!(truncate(absolu), annonce, "numéro non congruent accepté");
            assert_eq!(resolve(absolu, annonce), absolu, "aller-retour infidèle");
            // 2. Le paquet n'est ni transféré ni en cours de transfert.
            assert!(
                geo.first_frame_of(absolu) >= position,
                "paquet {absolu} accepté alors que la position est à {position}"
            );
            // 3. Il n'a pas déjà été accepté, et il tient dans le tampon.
            if let Some(precedent) = dernier {
                assert!(absolu > precedent, "paquet {absolu} accepté deux fois");
            }
            assert!(
                absolu < geo.packet_of(position).saturating_add(geo.packets_per_buffer()),
                "paquet {absolu} accepté hors du tampon"
            );
            // La borne que le plan de copie utilisera : elle ne peut pas être sous la
            // position, sans quoi la copie ne lirait rien de ce paquet.
            assert!(geo.end_frame_of(absolu) > position || geo.end_frame_of(absolu) == u64::MAX);
            Some(absolu)
        }
        WriteVerdict::Late | WriteVerdict::Overrun => None,
    }
}
