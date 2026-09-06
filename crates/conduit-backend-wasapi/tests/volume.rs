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
