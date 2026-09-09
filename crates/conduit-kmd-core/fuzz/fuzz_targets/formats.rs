//! Cible cargo-fuzz : le catalogue de formats d'un câble, l'arithmétique de variantes et
//! le dimensionnement du tampon cyclique (M1b-08, remise à jour en M1b-21).
//!
//! Trois chemins d'entrée, tous nourris par un tiers :
//!
//! - `CableFormat::sanitize` reçoit un `REG_DWORD` que l'administrateur écrit dans
//!   `regedit`, et [`cable_formats`] en tire la liste des formats que le câble déclare ;
//! - `validate` reçoit un `WAVEFORMATEXTENSIBLE` recopié depuis une requête KS, c'est-à-dire
//!   des nombres que le moteur audio choisit ;
//! - `buffer_bytes` reçoit une taille demandée par `AllocateBufferWithNotification`.
//!
//! Sur ce chemin, une panique bloque le démarrage du flux — ou pire, le système.
//!
//! # Ce que la cible vise
//!
//! **Les fonctions d'entrée du module, pas la disposition de ses tables.** La version
//! d'origine recopiait `M1A_FORMATS`, une `const` que M1b-05 a supprimée en rendant le jeu
//! de formats calculé à l'exécution : la cible a cessé de compiler sans que rien ne le dise
//! (les crates `fuzz/` sont hors du workspace). Elle n'appelle donc plus que des fonctions
//! publiques — [`cable_formats`], [`variant_of`], [`variant_rate`], [`variant_channels`],
//! [`CableFormat::sanitize`] — et lit toutes ses bornes dans le crate.
//!
//! # Disposition de l'entrée
//!
//! | Décalage | Champ |
//! |---|---|
//! | `0..4` | `nSamplesPerSec` demandé (u32 petit-boutiste) |
//! | `4..6` | `nChannels` |
//! | `6..8` | `wBitsPerSample` |
//! | `8..10` | `wValidBitsPerSample` |
//! | `10` | famille : 0 = PCM, 1 = flottant, tout le reste = autre sous-format |
//! | `11..15` | taille de tampon demandée, en octets |
//! | `15..19` | taille d'une trame, en octets |
//! | `19..23` | fréquence servant au calcul du tampon |
//! | `23..27` | nombre de notifications par tour |
//! | `27..31` | plancher en millisecondes (le paramètre de registre `BufferMs`) |
//! | `31..35` | fréquence du câble, libre |
//! | `35` | nombre de canaux du câble, libre (zéro compris) |
//! | `36` | index de variante de descripteurs, brut |
//! | `37..41` | encodage `REG_DWORD` d'un `CableFormat<n>` |
//!
//! Un champ absent vaut zéro : une entrée courte reste une entrée valide.
//!
//! # Ce qui est vérifié
//!
//! - **le registre ne peut pas fabriquer un câble sans variante** : un `CableFormat` sorti
//!   de `sanitize` a toujours un rang de fréquence et un index de variante, et son encodage
//!   se relit à l'identique ;
//! - **l'arithmétique de variantes est bijective** sur son domaine : `variant_of` et le
//!   couple `variant_rate`/`variant_channels` sont réciproques, et hors domaine les deux
//!   disent `None` **ensemble** ;
//! - `validate` ne rend que des formats **présents dans la liste** et qui **acceptent** la
//!   demande ; un refus signifie qu'aucune entrée ne l'accepte ;
//! - une liste issue de `cable_formats` avec des canaux **valides** ne rend jamais un
//!   format sans disposition de trame — c'est la garde ajoutée par M1b-08 ;
//! - le contrat de `buffer_bytes` : la taille rendue est un multiple entier de trame,
//!   **jamais inférieure à la demande** (écrêter serait pire qu'un refus), au moins le
//!   plancher demandé et sous le plafond de `MAX_BUFFER_MS` ;
//! - celui de `buffer_bytes_for_notifications` : au moins autant que `buffer_bytes`, et un
//!   nombre de trames divisible par le nombre de notifications ;
//! - que les deux formes par défaut valent bien leur forme « à plancher » prise à
//!   `MIN_BUFFER_MS`.
//!
//! `cargo +nightly fuzz run formats` depuis `crates/conduit-kmd-core`.
#![no_main]

use conduit_kmd_core::config::CableFormat;
use conduit_kmd_core::format::{
    buffer_bytes, buffer_bytes_for_notifications, buffer_bytes_for_notifications_with_floor,
    buffer_bytes_with_floor, cable_formats, sample_rate_index, validate, variant_channels,
    variant_index, variant_of, variant_rate, RequestedFormat, SampleKind, SupportedFormat,
    MAX_BUFFER_MS, MAX_CHANNELS_PER_CABLE, MIN_BUFFER_MS, VARIANT_COUNT,
};
use libfuzzer_sys::fuzz_target;

/// Lit un `u32` petit-boutiste, ou zéro si l'entrée s'arrête avant.
fn mot(data: &[u8], offset: usize) -> u32 {
    match data.get(offset..offset.saturating_add(4)) {
        Some([a, b, c, d]) => u32::from_le_bytes([*a, *b, *c, *d]),
        _ => 0,
    }
}

/// Lit un `u16` petit-boutiste, ou zéro si l'entrée s'arrête avant.
fn demi_mot(data: &[u8], offset: usize) -> u16 {
    match data.get(offset..offset.saturating_add(2)) {
        Some([a, b]) => u16::from_le_bytes([*a, *b]),
        _ => 0,
    }
}

/// Lit un octet, ou zéro si l'entrée s'arrête avant.
fn octet(data: &[u8], offset: usize) -> u8 {
    data.get(offset).copied().unwrap_or(0)
}

/// Le contrat de `validate` sur une liste quelconque : ce qui est retenu est dans la liste
/// et accepte la demande ; ce qui est refusé n'est accepté par personne.
///
/// `trame_exigee` dit si les entrées de la liste ont toutes une disposition de trame — ce
/// qui est le cas d'un `cable_formats` construit sur des canaux valides, et pas d'un
/// `cable_formats` bâti sur `channels == 0`.
fn contrat_de_validate(
    demande: &RequestedFormat,
    supportes: &[SupportedFormat],
    trame_exigee: bool,
) {
    match validate(demande, supportes) {
        Ok(retenu) => {
            assert!(
                supportes.contains(&retenu),
                "format retenu absent de la liste"
            );
            assert!(retenu.accepts(demande), "format retenu qui n'accepte pas");
            if trame_exigee {
                assert!(
                    retenu.layout().is_some(),
                    "format retenu sans disposition de trame"
                );
            }
        }
        Err(cause) => {
            assert!(
                !supportes.iter().any(|s| s.accepts(demande)),
                "refus alors qu'une entrée accepte la demande"
            );
            // Cette cause part dans le journal du pilote : la rendre ne doit pas paniquer.
            assert!(!cause.to_string().is_empty());
        }
    }
}

fuzz_target!(|data: &[u8]| {
    // ---------------------------------------------------------------------------
    // 1. Le registre : un `REG_DWORD` arbitraire vers un format de câble.
    // ---------------------------------------------------------------------------
    let brut = mot(data, 37);
    let (du_registre, correction) = CableFormat::sanitize(0, brut);
    // Le contrat que le pilote tient pour acquis : un format sorti de `sanitize` a
    // toujours une rangée dans les tables de descripteurs.
    assert!(
        du_registre.rate_index().is_some(),
        "format du registre sans rang de fréquence : {du_registre:?}"
    );
    let variante_du_registre = du_registre
        .variant()
        .expect("format du registre sans variante de descripteurs");
    assert!(variante_du_registre < VARIANT_COUNT);
    match correction {
        None => assert_eq!(
            CableFormat::decode(brut),
            Ok(du_registre),
            "sanitize a retenu un format que decode ne rend pas"
        ),
        Some(fix) => {
            assert_eq!(fix.found, brut);
            assert_eq!(fix.applied, du_registre.encode());
            // Cette ligne part dans le journal d'événements.
            assert!(!fix.to_string().is_empty());
        }
    }
    // L'encodage se relit à l'identique, et sans correction cette fois.
    let (relu, rien) = CableFormat::sanitize(0, du_registre.encode());
    assert_eq!(relu, du_registre, "aller-retour d'encodage infidèle");
    assert!(
        rien.is_none(),
        "sanitize corrige ce qu'elle vient de rendre"
    );

    // ---------------------------------------------------------------------------
    // 2. L'arithmétique de variantes, dans les deux sens.
    // ---------------------------------------------------------------------------
    let variante = usize::from(octet(data, 36));
    match (variant_rate(variante), variant_channels(variante)) {
        (Some(rate), Some(channels)) => {
            assert!(variante < VARIANT_COUNT, "variante hors compte acceptée");
            assert!(
                channels >= 1 && usize::from(channels) <= MAX_CHANNELS_PER_CABLE,
                "variante de {channels} canaux"
            );
            assert_eq!(
                variant_of(rate, channels),
                Some(variante),
                "aller-retour de variante infidèle"
            );
        }
        (None, None) => assert!(
            variante >= VARIANT_COUNT,
            "variante {variante} sans fréquence ni canaux sous le compte {VARIANT_COUNT}"
        ),
        (rate, channels) => panic!(
            "variant_rate et variant_channels en désaccord sur le domaine : \
             {rate:?} / {channels:?}"
        ),
    }

    // Le même aller-retour depuis un couple libre, celui-là hors domaine la plupart du
    // temps : c'est ce qui éprouve le refus plutôt que le repli muet.
    let rate_libre = mot(data, 31);
    let canaux_libres = octet(data, 35);
    match variant_of(rate_libre, canaux_libres) {
        Some(index) => {
            assert!(index < VARIANT_COUNT);
            assert_eq!(variant_rate(index), Some(rate_libre));
            assert_eq!(variant_channels(index), Some(canaux_libres));
            let rang = sample_rate_index(rate_libre).expect("variante sans rang de fréquence");
            assert_eq!(variant_index(rang, canaux_libres), Some(index));
        }
        None => assert!(
            sample_rate_index(rate_libre).is_none()
                || canaux_libres == 0
                || usize::from(canaux_libres) > MAX_CHANNELS_PER_CABLE,
            "{rate_libre} Hz sur {canaux_libres} canaux refusé alors qu'il est dans le domaine"
        ),
    }

    // ---------------------------------------------------------------------------
    // 3. `validate`, sur trois listes : celle du registre, une libre, et leur mélange.
    // ---------------------------------------------------------------------------
    let demande = RequestedFormat {
        sample_rate: mot(data, 0),
        channels: demi_mot(data, 4),
        bits_per_sample: demi_mot(data, 6),
        valid_bits: demi_mot(data, 8),
        kind: match octet(data, 10) {
            0 => SampleKind::Pcm,
            1 => SampleKind::Float,
            _ => SampleKind::Other,
        },
    };

    // La liste que le pilote déclare vraiment : celle du format lu au registre. Ses canaux
    // sont dans les bornes, donc toutes ses entrées ont une trame.
    let du_cable = cable_formats(du_registre.sample_rate, du_registre.channels);
    assert!(
        du_cable.iter().all(|f| f.layout().is_some()),
        "le catalogue du registre porte une entrée sans trame"
    );
    contrat_de_validate(&demande, &du_cable, true);

    // Une liste bâtie sur un couple libre : `channels == 0` y donne des entrées sans
    // trame, que `accepts` doit refuser d'elle-même.
    let libre = cable_formats(rate_libre, canaux_libres);
    let trame_partout = usize::from(canaux_libres) <= MAX_CHANNELS_PER_CABLE && canaux_libres != 0;
    contrat_de_validate(&demande, &libre, trame_partout);

    let melange: Vec<_> = du_cable.iter().chain(libre.iter()).copied().collect();
    contrat_de_validate(&demande, &melange, false);

    // ---------------------------------------------------------------------------
    // 4. Le dimensionnement du tampon cyclique.
    // ---------------------------------------------------------------------------
    let demandes_octets = mot(data, 11);
    let octets_par_trame = mot(data, 15);
    let frequence = mot(data, 19);
    let notifications = mot(data, 23);
    let plancher_ms = mot(data, 27);
    // Le plancher que la fonction retient vraiment : elle écrête ce qu'on lui donne.
    let plancher_retenu = plancher_ms.clamp(MIN_BUFFER_MS, MAX_BUFFER_MS);

    // Les deux formes par défaut sont bien les formes « à plancher » prises à MIN_BUFFER_MS.
    assert_eq!(
        buffer_bytes(demandes_octets, octets_par_trame, frequence),
        buffer_bytes_with_floor(demandes_octets, octets_par_trame, frequence, MIN_BUFFER_MS),
        "buffer_bytes diverge de son plancher par défaut"
    );
    assert_eq!(
        buffer_bytes_for_notifications(demandes_octets, octets_par_trame, frequence, notifications),
        buffer_bytes_for_notifications_with_floor(
            demandes_octets,
            octets_par_trame,
            frequence,
            notifications,
            MIN_BUFFER_MS
        ),
        "buffer_bytes_for_notifications diverge de son plancher par défaut"
    );

    let taille = buffer_bytes_with_floor(demandes_octets, octets_par_trame, frequence, plancher_ms);
    if let Some(octets) = taille {
        // La fonction ne rend `Some` que si la trame et la fréquence sont non nulles.
        assert_ne!(octets_par_trame, 0);
        assert_ne!(frequence, 0);
        assert_eq!(
            octets % octets_par_trame,
            0,
            "taille non alignée sur la trame"
        );
        assert!(
            octets >= demandes_octets,
            "tampon plus petit que demandé : {octets} < {demandes_octets}"
        );
        let trames = u64::from(octets / octets_par_trame);
        assert!(trames >= 1, "tampon vide");
        // Le plancher du contrat : ceil(fréquence × plancher_retenu / 1000) trames.
        let plancher = (u64::from(frequence) * u64::from(plancher_retenu)).div_ceil(1_000);
        assert!(
            trames >= plancher,
            "tampon de {trames} trames sous le plancher {plancher}"
        );
        // Le plafond du contrat : floor(fréquence × MAX_BUFFER_MS / 1000) trames.
        let plafond = u64::from(frequence) * u64::from(MAX_BUFFER_MS) / 1_000;
        assert!(
            trames <= plafond,
            "tampon de {trames} trames au-dessus du plafond {plafond}"
        );
    }

    if let Some(alignee) = buffer_bytes_for_notifications_with_floor(
        demandes_octets,
        octets_par_trame,
        frequence,
        notifications,
        plancher_ms,
    ) {
        let base = taille.expect("l'alignement a réussi là où le dimensionnement a échoué");
        assert_ne!(notifications, 0);
        assert!(alignee >= base, "l'alignement a rétréci le tampon");
        assert_eq!(
            alignee % octets_par_trame,
            0,
            "taille non alignée sur la trame"
        );
        assert_eq!(
            (alignee / octets_par_trame) % notifications,
            0,
            "période de notification non entière en trames"
        );
    }
});
