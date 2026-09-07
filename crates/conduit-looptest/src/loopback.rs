//! La vraie boucle, sur Windows : un flux de rendu qui joue le sinus, un flux de
//! capture qui enregistre ce qui revient.
//!
//! Rien n'est réécrit de WASAPI : tout passe par `conduit-backend-wasapi`
//! (`WasapiBackend::new`, `devices`, `open`, `open_loopback`,
//! `DeviceHandle::{start, stop}`).
//!
//! En **capture d'écho** (`--loopback`), l'endpoint de capture n'est pas ouvert du
//! tout : le second flux est un écho de l'endpoint de **rendu** lui-même, qui
//! prélève le mélange du moteur audio **avant** le pilote. Le flux est alors ouvert
//! au **format de mixage** de cet endpoint — c'est exactement ce que le moteur
//! mélange, sans conversion — et le sinus est joué à ce format-là.
//!
//! Le rappel de capture **n'alloue pas et ne verrouille pas** : l'enregistrement
//! vit dans un tableau d'`AtomicU32` réservé à l'ouverture (un `f32` par case, par
//! `to_bits`), écrit par index, avec un compteur atomique de trames. C'est la
//! seule façon d'écrire depuis un rappel temps réel sans `unsafe` ni verrou.
//!
//! Le rappel de rendu, lui, garde sa phase entre deux blocs (`FnMut`) et appelle
//! [`analysis::fill`], qui n'alloue pas non plus.

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;

use conduit_backend::{
    AudioCallback, Backend, DeviceDirection, DeviceHandle, DeviceInfo, StreamFormat, StreamIo,
};
use conduit_backend_wasapi::{EndpointVolumeControl, WasapiBackend};
use conduit_core::types::SampleRate;

use crate::analysis::{self, SineSpec};
use crate::cli::Args;
use crate::volume::{Level, Range, RangeState, Reading, State};

/// Préfixe du nom des endpoints du câble Conduit (`devices::cable_id_from_name`).
const CABLE_PREFIX: &str = "Conduit 1";

/// Marge de tampon d'enregistrement, en secondes : la capture tourne un peu plus
/// longtemps que le rendu, et un paquet WASAPI peut déborder la durée demandée.
const RECORD_MARGIN_S: f64 = 1.0;

/// Temps laissé à la capture après l'arrêt du rendu, pour ramasser la fin.
const DRAIN: Duration = Duration::from_millis(50);

/// Enregistrement rempli depuis le rappel de capture, sans allocation ni verrou.
#[derive(Debug)]
struct Recorder {
    /// Un `f32` par case (`f32::to_bits`), réservé à l'ouverture.
    data: Box<[AtomicU32]>,
    /// Trames déjà écrites.
    frames: AtomicUsize,
    /// Canaux du flux.
    channels: usize,
    /// Vrai si le tampon a débordé (la passe est alors trop longue).
    overflow: AtomicBool,
}

impl Recorder {
    fn new(frames: usize, channels: usize) -> Arc<Self> {
        let data = (0..frames * channels)
            .map(|_| AtomicU32::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Arc::new(Self {
            data,
            frames: AtomicUsize::new(0),
            channels,
            overflow: AtomicBool::new(false),
        })
    }

    /// Écrit un paquet capturé. Appelé depuis le fil temps réel du flux.
    fn push(&self, input: &[f32]) {
        let start = self.frames.load(Ordering::Relaxed) * self.channels;
        if start + input.len() > self.data.len() {
            self.overflow.store(true, Ordering::Relaxed);
            return;
        }
        for (slot, sample) in self.data[start..start + input.len()].iter().zip(input) {
            slot.store(sample.to_bits(), Ordering::Relaxed);
        }
        self.frames.store(
            start / self.channels + input.len() / self.channels,
            Ordering::Relaxed,
        );
    }

    /// Copie ce qui a été enregistré (hors temps réel).
    fn take(&self) -> Vec<f32> {
        let end = self.frames.load(Ordering::Relaxed) * self.channels;
        self.data[..end]
            .iter()
            .map(|slot| f32::from_bits(slot.load(Ordering::Relaxed)))
            .collect()
    }
}

/// Backend ouvert et endpoints choisis : réutilisés d'une passe à l'autre.
#[derive(Debug)]
pub struct Session {
    backend: WasapiBackend,
    render: DeviceInfo,
    capture: Option<DeviceInfo>,
    /// Vrai en `--loopback` : la capture est un écho de `render`, pas un endpoint
    /// à part.
    loopback: bool,
    format: StreamFormat,
    spec: SineSpec,
    seconds: f64,
}

impl Session {
    /// Ouvre le backend et résout les endpoints.
    ///
    /// # Erreurs
    ///
    /// Message en français si le backend ne démarre pas ou si un endpoint manque
    /// (l'appelant en fait un code de retour 2).
    pub fn open(args: &Args, rate: SampleRate) -> Result<Self, String> {
        let backend = WasapiBackend::new().map_err(|e| {
            format!("backend WASAPI indisponible : {e} — le service audio Windows tourne-t-il ?")
        })?;
        let devices = enumerate(&backend)?;
        let render = choose(&devices, DeviceDirection::Render, args.render.as_deref())?;
        let capture = if args.loopback || args.capture_disabled() {
            None
        } else {
            Some(choose(
                &devices,
                DeviceDirection::Capture,
                args.capture.as_deref(),
            )?)
        };
        // En écho, le format n'est pas négociable : c'est celui que le moteur
        // mélange vers cet endpoint. Le demander tel quel évite toute conversion
        // sur le chemin qu'on est justement en train de mettre en doute.
        let (rate, channels) = if args.loopback {
            (render.sample_rate, render.channels)
        } else {
            (rate, args.channels)
        };
        let spec = SineSpec {
            sample_rate: rate.hz(),
            channels,
            ..args.spec()
        };
        if !(spec.freq_hz > 0.0 && spec.freq_hz < rate.as_f64() / 2.0) {
            return Err(format!(
                "--freq {} doit être dans ]0, {}[ Hz : la capture en écho impose le format de \
                 mixage de « {} », soit {} Hz",
                spec.freq_hz,
                rate.as_f64() / 2.0,
                render.name,
                rate.hz()
            ));
        }
        Ok(Self {
            backend,
            render,
            capture,
            loopback: args.loopback,
            format: StreamFormat {
                sample_rate: rate,
                channels,
                block_frames: args.block,
            },
            spec,
            seconds: args.seconds,
        })
    }

    /// Volume, coupure et plage en décibels des endpoints que la mesure va
    /// utiliser.
    ///
    /// Relevé **avant** la première passe : un endpoint coupé ou à zéro rend toute
    /// mesure silencieuse, et cela ressemble trait pour trait à un pilote muet. En
    /// écho, l'endpoint de rendu est aussi celui qui est écouté : un seul relevé.
    ///
    /// Une lecture qui échoue devient un [`State::Unreadable`], jamais une erreur :
    /// un diagnostic ne doit pas empêcher la mesure qu'il commente.
    ///
    /// La plage (`GetVolumeRange`) s'y ajoute : elle ne sert pas au diagnostic mais
    /// à la mesure — c'est l'échelle en décibels que le pilote de l'endpoint
    /// déclare. Comme le reste de ce relevé, elle n'ouvre aucun flux.
    pub fn levels(&self) -> Vec<Reading> {
        let mut endpoints = vec![("rendu", &self.render)];
        if let Some(capture) = &self.capture {
            endpoints.push(("capture", capture));
        }
        let control = match EndpointVolumeControl::new() {
            Ok(control) => control,
            Err(e) => {
                return endpoints
                    .into_iter()
                    .map(|(role, device)| {
                        Reading::new(role, &device.name, State::Unreadable(e.to_string()))
                    })
                    .collect()
            }
        };
        endpoints
            .into_iter()
            .map(|(role, device)| {
                Reading::new(role, &device.name, read(&control, &device.id))
                    .with_range(read_range(&control, &device.id))
            })
            .collect()
    }

    /// Sinus tel qu'il sera joué **et** analysé : en écho, sa fréquence
    /// d'échantillonnage et son nombre de canaux sont ceux du mixage de l'endpoint
    /// de rendu, pas ceux demandés en ligne de commande.
    pub fn spec(&self) -> SineSpec {
        self.spec
    }

    /// Vrai si un enregistrement est fait (capture ordinaire ou écho).
    fn records(&self) -> bool {
        self.loopback || self.capture.is_some()
    }

    /// Décrit les endpoints retenus et ce qui est mesuré.
    pub fn description(&self) -> String {
        if self.loopback {
            return format!(
                "rendu   : {} ({})\n\
                 capture : écho de ce même endpoint de rendu (WASAPI loopback)\n\
                 format  : {} Hz, {} canal/canaux — le format de mixage du moteur pour cet \
                 endpoint\n\
                 mesuré  : ce que le moteur audio de Windows délivre vers cet endpoint, prélevé \
                 AVANT le pilote.\n\
                 \x20         Le mélange contient aussi ce que jouent les autres applications : \
                 fermez-les pour une mesure propre.",
                self.render.name,
                self.render.id,
                self.format.sample_rate.hz(),
                self.format.channels,
            );
        }
        format!(
            "rendu   : {} ({})\ncapture : {}",
            self.render.name,
            self.render.id,
            match &self.capture {
                Some(d) => format!("{} ({})", d.name, d.id),
                None => "aucune (--no-capture)".to_string(),
            }
        )
    }

    /// Joue le sinus et enregistre ce qui revient : une passe.
    ///
    /// La capture démarre **avant** le rendu, pour ne pas manquer l'attaque ; le
    /// préambule qu'elle enregistre est jeté à l'analyse. En écho, « la capture »
    /// est l'écho de l'endpoint de rendu.
    ///
    /// # Erreurs
    ///
    /// Message en français si l'ouverture, le démarrage ou l'arrêt d'un flux
    /// échoue, ou si le tampon d'enregistrement a débordé.
    pub fn record(&mut self) -> Result<Vec<f32>, String> {
        let channels = self.format.channels;
        let capacity = if self.records() {
            ((self.seconds + RECORD_MARGIN_S) * self.format.sample_rate.as_f64()).ceil() as usize
        } else {
            0
        };
        let recorder = Recorder::new(capacity, channels);

        let sink = Arc::clone(&recorder);
        let sink: AudioCallback = Box::new(move |io: &mut StreamIo<'_>, _| {
            if let Some(input) = io.input {
                sink.push(input);
            }
            io.silence_output();
        });
        let mut capture: Option<Box<dyn DeviceHandle>> = if self.loopback {
            Some(Box::new(
                self.backend
                    .open_loopback(&self.render.id, self.format, sink)
                    .map_err(|e| {
                        format!("ouverture de l'écho de « {} » : {e}", self.render.name)
                    })?,
            ))
        } else {
            match &self.capture {
                Some(info) => Some(
                    self.backend
                        .open(&info.id, self.format, sink)
                        .map_err(|e| format!("ouverture de la capture « {} » : {e}", info.name))?,
                ),
                None => None,
            }
        };

        let spec = self.spec;
        let mut phase = 0.0f64;
        let mut render = self
            .backend
            .open(
                &self.render.id,
                self.format,
                Box::new(move |io: &mut StreamIo<'_>, _| {
                    if let Some(out) = io.output.as_deref_mut() {
                        phase = analysis::fill(&spec, out, phase);
                    }
                }),
            )
            .map_err(|e| format!("ouverture du rendu « {} » : {e}", self.render.name))?;

        if let Some(capture) = capture.as_mut() {
            capture
                .start()
                .map_err(|e| format!("démarrage de la capture : {e}"))?;
        }
        render
            .start()
            .map_err(|e| format!("démarrage du rendu : {e}"))?;
        sleep(Duration::from_secs_f64(self.seconds));
        if self.loopback {
            // En écho, l'enregistrement s'arrête **avant** le rendu. Il n'y a aucun
            // délai de pilote à drainer — le mélange est prélevé là où le moteur
            // vient de l'écrire —, et laisser tourner l'écho après l'arrêt du rendu
            // n'ajouterait qu'une queue de silence, que l'analyse compterait à juste
            // titre comme un trou.
            if let Some(capture) = capture.as_mut() {
                capture
                    .stop()
                    .map_err(|e| format!("arrêt de l'écho : {e}"))?;
            }
            render.stop().map_err(|e| format!("arrêt du rendu : {e}"))?;
        } else {
            render.stop().map_err(|e| format!("arrêt du rendu : {e}"))?;
            if let Some(capture) = capture.as_mut() {
                sleep(DRAIN);
                capture
                    .stop()
                    .map_err(|e| format!("arrêt de la capture : {e}"))?;
            }
        }

        if recorder.overflow.load(Ordering::Relaxed) {
            return Err(format!(
                "le tampon d'enregistrement ({capacity} trames) a débordé — réduisez --seconds"
            ));
        }
        Ok(recorder.take())
    }
}

/// Énumère les endpoints actifs.
///
/// # Erreurs
///
/// Message en français si l'énumération échoue.
pub fn enumerate(backend: &WasapiBackend) -> Result<Vec<DeviceInfo>, String> {
    backend
        .devices()
        .map_err(|e| format!("énumération des endpoints : {e}"))
}

/// Relève le volume d'un endpoint, sans jamais échouer : une panne devient un
/// [`State::Unreadable`], une absence de mélangeur un [`State::NoControl`].
fn read(control: &EndpointVolumeControl, id: &conduit_backend::DeviceId) -> State {
    match control.read(id) {
        Ok(Some(volume)) => State::Known(Level {
            scalar: volume.scalar,
            muted: volume.muted,
        }),
        Ok(None) => State::NoControl,
        Err(e) => State::Unreadable(e.to_string()),
    }
}

/// Relève la plage en décibels d'un endpoint, sans jamais échouer ni rien ouvrir.
///
/// `GetVolumeRange` est une interrogation en lecture seule de l'endpoint : aucun
/// flux n'est ouvert, aucun son n'est émis. Un endpoint qui n'annonce pas sa plage
/// donne un [`RangeState::NoControl`], une panne un [`RangeState::Unreadable`] —
/// jamais une erreur : la plage renseigne, elle ne commande rien.
fn read_range(control: &EndpointVolumeControl, id: &conduit_backend::DeviceId) -> RangeState {
    match control.read_range(id) {
        Ok(Some(range)) => RangeState::Known(Range {
            min_db: range.min_db,
            max_db: range.max_db,
            increment_db: range.increment_db,
        }),
        Ok(None) => RangeState::NoControl,
        Err(e) => RangeState::Unreadable(e.to_string()),
    }
}

/// Affiche les endpoints (`--list`), avec leur volume si `show_volume`.
///
/// # Erreurs
///
/// Message en français si le backend, l'énumération ou — quand les volumes sont
/// demandés — le contrôle du volume échoue.
pub fn list(show_volume: bool) -> Result<String, String> {
    let backend = WasapiBackend::new().map_err(|e| format!("backend WASAPI indisponible : {e}"))?;
    let devices = enumerate(&backend)?;
    // Demandé explicitement : une panne du contrôle est ici une vraie erreur, pas
    // une ligne manquante qu'on laisserait passer inaperçue.
    let control = show_volume
        .then(EndpointVolumeControl::new)
        .transpose()
        .map_err(|e| format!("contrôle du volume indisponible : {e}"))?;
    let mut out = format!("{} endpoint(s) actif(s) :\n", devices.len());
    for device in &devices {
        out.push_str(&format!(
            "  [{}] {}{}{}\n      {} canaux à {} Hz, bloc {} trames\n",
            device.direction,
            device.name,
            if device.is_default { " (défaut)" } else { "" },
            if device.name.starts_with(CABLE_PREFIX) {
                " ← câble Conduit"
            } else {
                ""
            },
            device.channels,
            device.sample_rate.hz(),
            device.default_block,
        ));
        if let Some(control) = &control {
            let reading = Reading::new(
                device.direction.to_string(),
                &device.name,
                read(control, &device.id),
            )
            .with_range(read_range(control, &device.id));
            out.push_str(&format!("      volume {}\n", reading.describe()));
            if let Some(range) = reading.range_line() {
                out.push_str(&format!("      {range}\n"));
            }
        }
        out.push_str(&format!("      id {}\n", device.id));
    }
    if !devices.iter().any(|d| d.name.starts_with(CABLE_PREFIX)) {
        out.push_str(&format!(
            "\nAucun endpoint « {CABLE_PREFIX} … » : le pilote Conduit n'est pas chargé sur cette \
             machine (voir docs/driver-dev.md). Le test de boucle demande alors --render et \
             --capture explicites.\n"
        ));
    }
    Ok(out)
}

/// Règle le volume et la coupure des endpoints de `--render` et `--capture`
/// (`--set-volume`, `--unmute`), et rend le compte rendu à imprimer.
///
/// Rien n'est joué : régler un volume ne doit pas avoir pour effet de bord d'émettre
/// du son. L'état est affiché **avant et après**, pour que le réglage se vérifie
/// lui-même — c'est le seul moyen de distinguer « réglé » de « refusé sans le dire ».
///
/// # Erreurs
///
/// Message en français si le backend, l'énumération, le contrôle du volume ou
/// l'écriture échouent, ou si un endpoint désigné n'existe pas.
pub fn adjust_volume(args: &Args) -> Result<String, String> {
    let backend = WasapiBackend::new().map_err(|e| format!("backend WASAPI indisponible : {e}"))?;
    let devices = enumerate(&backend)?;
    let mut endpoints = vec![(
        "rendu",
        choose(&devices, DeviceDirection::Render, args.render.as_deref())?,
    )];
    if !args.capture_disabled() {
        endpoints.push((
            "capture",
            choose(&devices, DeviceDirection::Capture, args.capture.as_deref())?,
        ));
    }
    drop(backend);

    let control = EndpointVolumeControl::new()
        .map_err(|e| format!("contrôle du volume indisponible : {e}"))?;
    let mut out = String::new();
    for (role, device) in &endpoints {
        let before = Reading::new(*role, &device.name, read(&control, &device.id));
        let mut after = before.state.clone();
        if let Some(scalar) = args.set_volume {
            after = write(
                control.set_scalar(&device.id, scalar),
                "réglage du volume",
                &device.name,
            )?;
        }
        if args.unmute {
            after = write(
                control.set_mute(&device.id, false),
                "rétablissement du son",
                &device.name,
            )?;
        }
        let after = Reading::new(*role, &device.name, after);
        out.push_str(&format!(
            "{} « {} »\n    avant : {}\n    après : {}\n",
            role,
            device.name,
            before.describe(),
            after.describe()
        ));
        if let Some(warning) = after.warning() {
            out.push_str(&warning);
            out.push('\n');
        }
    }
    Ok(out)
}

/// Traduit le résultat d'une écriture de volume : `Ok(None)` (pas de contrôle) est
/// une réponse, une erreur COM en est une aussi mais elle arrête l'action — on a
/// demandé un réglage, pas un diagnostic.
fn write(
    result: Result<Option<conduit_backend_wasapi::EndpointVolume>, conduit_backend::BackendError>,
    what: &str,
    name: &str,
) -> Result<State, String> {
    match result {
        Ok(Some(volume)) => Ok(State::Known(Level {
            scalar: volume.scalar,
            muted: volume.muted,
        })),
        Ok(None) => Ok(State::NoControl),
        Err(e) => Err(format!("{what} de « {name} » : {e}")),
    }
}

/// Choisit un endpoint : identifiant exact, sinon fragment de nom, sinon le câble.
///
/// # Erreurs
///
/// Message en français listant ce qui a été vu, quand rien ne correspond.
pub fn choose(
    devices: &[DeviceInfo],
    direction: DeviceDirection,
    wanted: Option<&str>,
) -> Result<DeviceInfo, String> {
    let candidates: Vec<&DeviceInfo> = devices
        .iter()
        .filter(|d| d.direction == direction)
        .collect();
    let names = || {
        if candidates.is_empty() {
            "aucun".to_string()
        } else {
            candidates
                .iter()
                .map(|d| format!("« {} »", d.name))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };
    match wanted {
        Some(want) => {
            let lower = want.to_lowercase();
            candidates
                .iter()
                .find(|d| d.id.as_str() == want)
                .or_else(|| {
                    candidates
                        .iter()
                        .find(|d| d.name.to_lowercase().contains(&lower))
                })
                .map(|d| (*d).clone())
                .ok_or_else(|| {
                    format!(
                        "aucun endpoint de {direction} ne correspond à « {want} » ; vus : {}. \
                         Lancez `conduit-looptest --list`.",
                        names()
                    )
                })
        }
        None => candidates
            .iter()
            .find(|d| d.name.starts_with(CABLE_PREFIX))
            .map(|d| (*d).clone())
            .ok_or_else(|| {
                format!(
                    "aucun endpoint de {direction} nommé « {CABLE_PREFIX} … » : le pilote Conduit \
                     n'est pas chargé (installez-le dans la VM, docs/driver-dev.md). Endpoints de \
                     {direction} vus : {}. Sinon, désignez des endpoints réels avec --render et \
                     --capture.",
                    names()
                )
            }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conduit_backend::DeviceId;

    fn device(name: &str, direction: DeviceDirection) -> DeviceInfo {
        DeviceInfo {
            id: DeviceId::new(format!("{{id-{name}}}")),
            name: name.to_string(),
            direction,
            channels: 2,
            sample_rate: SampleRate::HZ_48000,
            sample_rates: vec![],
            default_block: 480,
            is_default: false,
            cable: None,
        }
    }

    fn devices() -> Vec<DeviceInfo> {
        vec![
            device(
                "G27QC A (NVIDIA High Definition Audio)",
                DeviceDirection::Render,
            ),
            device("Conduit 1", DeviceDirection::Render),
            device("Conduit 1", DeviceDirection::Capture),
            device("Microphone (Realtek)", DeviceDirection::Capture),
        ]
    }

    #[test]
    fn cable_choisi_par_defaut() {
        let d = devices();
        assert_eq!(
            choose(&d, DeviceDirection::Render, None).unwrap().name,
            "Conduit 1"
        );
        assert_eq!(
            choose(&d, DeviceDirection::Capture, None).unwrap().name,
            "Conduit 1"
        );
    }

    #[test]
    fn sans_cable_le_message_dit_que_le_pilote_manque() {
        let d = vec![device("G27QC A", DeviceDirection::Render)];
        let err = choose(&d, DeviceDirection::Render, None).unwrap_err();
        assert!(err.contains("pilote Conduit"), "{err}");
        assert!(err.contains("G27QC A"), "{err}");
    }

    #[test]
    fn fragment_de_nom_insensible_a_la_casse() {
        let d = devices();
        assert!(choose(&d, DeviceDirection::Render, Some("g27qc"))
            .unwrap()
            .name
            .starts_with("G27QC"));
        assert!(choose(&d, DeviceDirection::Capture, Some("realtek")).is_ok());
    }

    #[test]
    fn identifiant_exact_prioritaire() {
        let d = devices();
        let chosen = choose(&d, DeviceDirection::Render, Some("{id-Conduit 1}")).unwrap();
        assert_eq!(chosen.name, "Conduit 1");
    }

    #[test]
    fn fragment_inconnu_liste_ce_qui_existe() {
        let err = choose(&devices(), DeviceDirection::Render, Some("zzz")).unwrap_err();
        assert!(err.contains("G27QC"), "{err}");
        assert!(err.contains("--list"), "{err}");
    }

    #[test]
    fn enregistreur_sans_allocation() {
        let rec = Recorder::new(4, 2);
        rec.push(&[0.5, -0.5, 1.0, -1.0]);
        rec.push(&[0.25, 0.25]);
        assert_eq!(rec.take(), vec![0.5, -0.5, 1.0, -1.0, 0.25, 0.25]);
        assert!(!rec.overflow.load(Ordering::Relaxed));
        rec.push(&[0.0; 8]);
        assert!(rec.overflow.load(Ordering::Relaxed));
        assert_eq!(rec.take().len(), 6);
    }
}
