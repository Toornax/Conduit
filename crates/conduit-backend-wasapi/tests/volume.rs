//! Volume d'endpoint sur les vraies cartes de la machine (`EndpointVolumeControl`).
//!
//! Ces tests **écrivent** sur le périphérique de rendu par défaut de la machine qui
//! les lance. La valeur initiale est donc relevée d'abord et rendue par une garde
//! `Drop`, y compris si une assertion échoue : à aucun moment la machine ne doit
//! rester avec un volume qu'on lui a mis. La coupure, elle, n'est jamais changée —
//! elle n'est réécrite qu'à **sa propre valeur**, ce qui éprouve l'appel sans
//! toucher à l'état.
//!
//! Aucun son n'est émis : rien n'ouvre de flux.
//!
//! [`la_plage_du_rendu_par_defaut_se_releve_en_decibels`] n'écrit rien du tout et
//! ne fait qu'interroger la carte ; il est `#[ignore]` parce qu'il n'a de sens que
//! sur du matériel réel, et qu'il sert à **lire** un relevé, pas à garder un
//! invariant.
//!
//! Tout se saute proprement s'il n'y a pas de périphérique de rendu par défaut, ou
//! si celui-ci n'expose pas de contrôle de volume.

#![cfg(windows)]

use conduit_backend::{Backend, BackendError, DeviceDirection, DeviceId};
use conduit_backend_wasapi::{EndpointVolume, EndpointVolumeControl, WasapiBackend};

/// Écart toléré entre le volume demandé et le volume relu : Windows range la
/// valeur sur les crans du périphérique.
const TOLERANCE: f32 = 0.03;

/// Rend son volume initial à l'endpoint, quoi qu'il arrive au test.
struct Restore<'a> {
    control: &'a EndpointVolumeControl,
    id: DeviceId,
    initial: EndpointVolume,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        let _ = self.control.set_scalar(&self.id, self.initial.scalar);
        let _ = self.control.set_mute(&self.id, self.initial.muted);
    }
}

/// Identifiant du rendu par défaut, ou `None` s'il n'y en a pas (test à sauter).
fn default_render() -> Option<DeviceId> {
    let backend = match WasapiBackend::new() {
        Ok(backend) => backend,
        Err(e) => {
            eprintln!("test sauté : backend WASAPI indisponible ({e})");
            return None;
        }
    };
    let id = backend.default_device(DeviceDirection::Render);
    if id.is_none() {
        eprintln!("test sauté : aucun périphérique de rendu par défaut");
    }
    id
}

#[test]
fn le_volume_du_rendu_par_defaut_se_lit_s_ecrit_et_se_restaure() {
    let Some(id) = default_render() else { return };
    let control = EndpointVolumeControl::new().expect("contrôle du volume");

    let Some(initial) = control.read(&id).expect("lecture du volume") else {
        eprintln!("test sauté : le rendu par défaut n'expose pas de contrôle de volume");
        return;
    };
    let guard = Restore {
        control: &control,
        id: id.clone(),
        initial,
    };

    // Une cible franchement différente de l'existante, pour qu'un « rien ne s'est
    // passé » ne puisse pas passer pour un succès.
    let target = if initial.scalar > 0.6 { 0.25 } else { 0.75 };
    let after = control
        .set_scalar(&id, target)
        .expect("écriture du volume")
        .expect("l'endpoint expose un contrôle de volume");
    assert!(
        (after.scalar - target).abs() <= TOLERANCE,
        "relu {} après avoir écrit {target}",
        after.scalar
    );
    assert_eq!(after.muted, initial.muted, "l'écriture du volume a coupé");

    // Relecture indépendante : la valeur tient au-delà du retour de `set_scalar`.
    let reread = control
        .read(&id)
        .expect("relecture")
        .expect("contrôle toujours là");
    assert!(
        (reread.scalar - target).abs() <= TOLERANCE,
        "relecture {} après avoir écrit {target}",
        reread.scalar
    );

    // La coupure est réécrite à sa propre valeur : l'appel est éprouvé, l'état de la
    // machine ne bouge pas.
    let same = control
        .set_mute(&id, initial.muted)
        .expect("écriture de la coupure")
        .expect("contrôle toujours là");
    assert_eq!(same.muted, initial.muted);

    drop(guard);
    let restored = control
        .read(&id)
        .expect("relecture après restauration")
        .expect("contrôle toujours là");
    assert!(
        (restored.scalar - initial.scalar).abs() <= TOLERANCE,
        "volume non restauré : {} au lieu de {}",
        restored.scalar,
        initial.scalar
    );
    assert_eq!(restored.muted, initial.muted);
}

/// La **plage** en décibels du rendu par défaut, telle que la carte la déclare.
///
/// C'est la mesure qui doit trancher l'échelle de `KSPROPERTY_AUDIO_VOLUMELEVEL`
/// que le pilote Conduit exposera : la documentation Microsoft et les constantes
/// de SYSVAD la disent en unités de 1/65536 dB (pas de 0,5 dB pour `0x8000`,
/// minimum de −96 dB pour `-96 * 0x10000`), mais ce dépôt ne tient pas une
/// affirmation non mesurée pour un fait. Sur une vraie carte, ce test imprime la
/// forme attendue d'un pilote qui fait les choses correctement ; le jour où le
/// nœud `KSNODETYPE_VOLUME` de Conduit existe, le même relevé sur l'endpoint
/// `Conduit 1` confirmera l'échelle ou l'infirmera.
///
/// Le test n'affirme donc **pas** la valeur attendue — il la relève et vérifie sa
/// seule cohérence interne. Rien n'est ouvert, rien n'est écrit, rien ne sonne :
/// `GetVolumeRange` est une interrogation en lecture seule.
#[test]
#[ignore = "interroge la carte son de la machine (lecture seule, aucun son) : à lancer à la main (cargo test … -- --ignored)"]
fn la_plage_du_rendu_par_defaut_se_releve_en_decibels() {
    let Some(id) = default_render() else { return };
    let control = EndpointVolumeControl::new().expect("contrôle du volume");

    let Some(range) = control.read_range(&id).expect("lecture de la plage") else {
        eprintln!("test sauté : le rendu par défaut n'annonce pas de plage de volume");
        return;
    };
    eprintln!(
        "plage du rendu par défaut : {} dB à {} dB, pas {} dB",
        range.min_db, range.max_db, range.increment_db
    );

    assert!(
        range.min_db.is_finite() && range.max_db.is_finite() && range.increment_db.is_finite(),
        "plage non finie : {range:?}"
    );
    assert!(
        range.min_db < range.max_db,
        "minimum au-dessus du maximum : {range:?}"
    );
    assert!(
        range.increment_db > 0.0,
        "pas nul ou négatif : {range:?} — le curseur n'aurait aucun cran"
    );
    assert!(
        range.increment_db <= range.max_db - range.min_db,
        "pas plus large que la plage entière : {range:?}"
    );
}

#[test]
fn le_volume_d_un_endpoint_inconnu_est_introuvable() {
    let control = match EndpointVolumeControl::new() {
        Ok(control) => control,
        Err(e) => {
            eprintln!("test sauté : COM ou MMDevice indisponible ({e})");
            return;
        }
    };
    let id = DeviceId::new("{0.0.0.00000000}.{00000000-0000-0000-0000-000000000000}");
    match control.read(&id) {
        Err(BackendError::NotFound(missing)) => assert_eq!(missing, id),
        other => panic!("attendu NotFound, obtenu {other:?}"),
    }
}
