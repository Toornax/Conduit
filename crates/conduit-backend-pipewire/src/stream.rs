//! Ouverture d'un `pw_stream` et rappel temps réel.
//!
//! Tout ce fichier tourne sur le fil de la boucle PipeWire, **sauf** `process`,
//! appelé depuis le fil temps réel de PipeWire. Le rappel n'alloue pas, ne prend
//! aucun verrou et ne journalise pas : tout est pré-alloué à l'ouverture et la
//! coordination avec `start`/`stop` passe par son champ `state`.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use conduit_backend::{
    AudioCallback, BackendError, ClockInfo, DeviceDirection, DeviceId, DeviceInfo, StreamFormat,
    StreamIo,
};
use pipewire as pw;
use pw::spa;
use pw::stream::{StreamFlags, StreamListener, StreamRc};

/// Octets d'un échantillon `f32`.
const SAMPLE_BYTES: usize = std::mem::size_of::<f32>();

/// Flux arrêté : le rappel de Conduit n'est pas appelé.
const STATE_STOPPED: u8 = 0;
/// Flux actif, aucun rappel en cours.
const STATE_ACTIVE: u8 = 1;
/// Rappel de Conduit en cours d'exécution.
const STATE_RUNNING: u8 = 2;

/// État partagé entre le fil de boucle, le fil temps réel et la poignée.
#[derive(Debug, Default)]
pub(crate) struct StreamShared {
    /// `STATE_STOPPED` / `STATE_ACTIVE` / `STATE_RUNNING`.
    state: AtomicU8,
    /// Trames écoulées depuis l'ouverture, au début du prochain rappel.
    position: AtomicU64,
    /// Horodatage du dernier rappel, nanosecondes monotones.
    timestamp_ns: AtomicU64,
    /// Trames du dernier rappel.
    frames: AtomicUsize,
    /// Le périphérique a disparu du registre.
    disconnected: AtomicBool,
}

impl StreamShared {
    /// Active le flux (idempotent).
    pub(crate) fn activate(&self) {
        let _ = self.state.compare_exchange(
            STATE_STOPPED,
            STATE_ACTIVE,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }

    /// Désactive le flux et **attend** la fin d'un rappel éventuellement en cours,
    /// pour tenir la garantie « aucun rappel après `stop` ».
    ///
    /// Appelé depuis le fil de boucle, jamais depuis le fil temps réel : l'attente
    /// active est bornée par la durée d'un rappel.
    pub(crate) fn deactivate(&self) {
        loop {
            match self.state.compare_exchange_weak(
                STATE_ACTIVE,
                STATE_STOPPED,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(STATE_STOPPED) => return,
                Err(_) => std::hint::spin_loop(),
            }
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        self.state.load(Ordering::Acquire) != STATE_STOPPED
    }

    pub(crate) fn mark_disconnected(&self) {
        self.disconnected.store(true, Ordering::Release);
    }

    pub(crate) fn is_disconnected(&self) -> bool {
        self.disconnected.load(Ordering::Acquire)
    }

    pub(crate) fn clock(&self) -> ClockInfo {
        ClockInfo {
            position: self.position.load(Ordering::Acquire),
            timestamp_ns: self.timestamp_ns.load(Ordering::Acquire),
            frames: self.frames.load(Ordering::Acquire),
        }
    }
}

/// Données possédées par le rappel `process`, pré-allouées à l'ouverture.
struct ProcessData {
    callback: AudioCallback,
    shared: Arc<StreamShared>,
    direction: DeviceDirection,
    channels: usize,
    block_frames: usize,
    /// Position courante, propriété exclusive du fil temps réel.
    position: u64,
    /// Dernier horodatage émis, pour garantir la monotonie.
    last_timestamp_ns: u64,
    /// Base de repli quand `pw_stream_get_time()` n'a rien d'utilisable.
    epoch: Instant,
}

impl ProcessData {
    /// Horodatage monotone du rappel courant.
    ///
    /// PipeWire publie `pw_time.now` (CLOCK_MONOTONIC) ; à défaut on retombe sur
    /// l'horloge monotone du processus. `max` garantit la monotonie même si la
    /// source change.
    fn timestamp(&mut self, stream: &pw::stream::Stream) -> u64 {
        let now = match stream.time() {
            Ok(t) if t.now() > 0 => t.now() as u64,
            _ => self.epoch.elapsed().as_nanos() as u64,
        };
        self.last_timestamp_ns = now.max(self.last_timestamp_ns);
        self.last_timestamp_ns
    }
}

/// Un flux ouvert, vivant sur le fil de boucle.
///
/// L'ordre des champs est l'ordre de destruction : l'écouteur doit être retiré
/// avant que le flux ne soit détruit.
pub(crate) struct OpenStream {
    _listener: StreamListener<ProcessData>,
    stream: StreamRc,
    pub(crate) device: DeviceId,
    pub(crate) shared: Arc<StreamShared>,
}

impl OpenStream {
    pub(crate) fn set_active(&self, active: bool) -> Result<(), BackendError> {
        if active {
            self.shared.activate();
        } else {
            // D'abord la barrière logicielle (aucun nouveau rappel de Conduit),
            // ensuite l'arrêt côté PipeWire.
            self.shared.deactivate();
        }
        self.stream.set_active(active).map_err(|e| {
            BackendError::Platform(format!(
                "PipeWire refuse de {} le flux {} : {e}",
                if active { "démarrer" } else { "arrêter" },
                self.device
            ))
        })
    }

    /// Déconnecte proprement le flux.
    pub(crate) fn close(&self) {
        self.shared.deactivate();
        let _ = self.stream.disconnect();
    }
}

impl core::fmt::Debug for OpenStream {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OpenStream")
            .field("device", &self.device)
            .finish()
    }
}

/// Sérialise le format demandé en `SPA_PARAM_EnumFormat` (f32 entrelacé).
fn format_pod(device: &DeviceId, format: StreamFormat) -> Result<Vec<u8>, BackendError> {
    let mut info = spa::param::audio::AudioInfoRaw::new();
    info.set_format(spa::param::audio::AudioFormat::F32LE);
    info.set_rate(format.sample_rate.hz());
    info.set_channels(format.channels as u32);
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: info.into(),
    };
    spa::pod::serialize::PodSerializer::serialize(
        Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )
    .map(|(cursor, _)| cursor.into_inner())
    .map_err(|e| BackendError::UnsupportedFormat {
        device: device.clone(),
        reason: format!("format audio non sérialisable : {e}"),
    })
}

/// Propriétés du nœud client créé pour ce flux.
fn stream_properties(info: &DeviceInfo, format: StreamFormat) -> pw::properties::PropertiesBox {
    let mut props = pw::properties::PropertiesBox::new();
    props.insert(*pw::keys::MEDIA_TYPE, "Audio");
    props.insert(
        *pw::keys::MEDIA_CATEGORY,
        match info.direction {
            DeviceDirection::Render => "Playback",
            DeviceDirection::Capture => "Capture",
        },
    );
    props.insert(*pw::keys::MEDIA_ROLE, "Music");
    props.insert(*pw::keys::NODE_NAME, format!("conduit.{}", info.id));
    props.insert(
        *pw::keys::NODE_LATENCY,
        format!("{}/{}", format.block_frames, format.sample_rate.hz()),
    );
    props.insert(*pw::keys::NODE_AUTOCONNECT, "true");
    props.insert(*pw::keys::TARGET_OBJECT, info.id.as_str());
    props
}

/// Ouvre un flux vers `info`. À appeler depuis le fil de boucle.
pub(crate) fn open(
    core: &pw::core::CoreRc,
    info: &DeviceInfo,
    format: StreamFormat,
    callback: AudioCallback,
    shared: Arc<StreamShared>,
) -> Result<OpenStream, BackendError> {
    if format.channels == 0 {
        return Err(BackendError::UnsupportedFormat {
            device: info.id.clone(),
            reason: "un flux doit avoir au moins un canal".into(),
        });
    }
    let values = format_pod(&info.id, format)?;
    let props = stream_properties(info, format);
    let stream =
        StreamRc::new(core.clone(), &format!("conduit.{}", info.id), props).map_err(|e| {
            BackendError::Platform(format!("création du flux PipeWire pour {} : {e}", info.id))
        })?;

    let data = ProcessData {
        callback,
        shared: Arc::clone(&shared),
        direction: info.direction,
        channels: format.channels,
        block_frames: format.block_frames.max(1),
        position: 0,
        last_timestamp_ns: 0,
        epoch: Instant::now(),
    };
    let listener = stream
        .add_local_listener_with_user_data(data)
        .process(process)
        .register()
        .map_err(|e| {
            BackendError::Platform(format!("écoute du flux PipeWire {} : {e}", info.id))
        })?;

    let direction = match info.direction {
        DeviceDirection::Render => spa::utils::Direction::Output,
        DeviceDirection::Capture => spa::utils::Direction::Input,
    };
    let pod =
        spa::pod::Pod::from_bytes(&values).ok_or_else(|| BackendError::UnsupportedFormat {
            device: info.id.clone(),
            reason: "format audio non représentable en pod SPA".into(),
        })?;
    let mut params = [pod];
    stream
        .connect(
            direction,
            None,
            // `INACTIVE` : aucun rappel avant `DeviceHandle::start`.
            StreamFlags::AUTOCONNECT
                | StreamFlags::MAP_BUFFERS
                | StreamFlags::RT_PROCESS
                | StreamFlags::INACTIVE,
            &mut params,
        )
        .map_err(|e| {
            BackendError::Platform(format!(
                "connexion du flux PipeWire à {} : {e} — le nœud existe-t-il encore ?",
                info.id
            ))
        })?;

    Ok(OpenStream {
        _listener: listener,
        stream,
        device: info.id.clone(),
        shared,
    })
}

/// Rappel `process` : **fil temps réel**, aucune allocation ni verrou.
fn process(stream: &pw::stream::Stream, data: &mut ProcessData) {
    let Some(mut buffer) = stream.dequeue_buffer() else {
        return;
    };
    let requested = buffer.requested() as usize;
    let stride = data.channels * SAMPLE_BYTES;
    let datas = buffer.datas_mut();
    if datas.is_empty() {
        return;
    }
    let plane = &mut datas[0];

    // Le rappel de Conduit n'est exécuté que si le flux est actif ; la transition
    // vers `STATE_RUNNING` bloque un `stop` concurrent jusqu'à la fin du rappel.
    let active = data
        .shared
        .state
        .compare_exchange(
            STATE_ACTIVE,
            STATE_RUNNING,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok();
    let timestamp_ns = data.timestamp(stream);

    let frames = match data.direction {
        DeviceDirection::Render => {
            let block = if requested > 0 {
                requested
            } else {
                data.block_frames
            };
            // Bloc séparé : la vue octets doit être relâchée avant `chunk_mut`.
            let frames = {
                let Some(bytes) = plane.data() else {
                    finish(data, active, 0, timestamp_ns);
                    return;
                };
                let frames = block.min(bytes.len() / stride);
                let Ok(samples) =
                    bytemuck::try_cast_slice_mut::<u8, f32>(&mut bytes[..frames * stride])
                else {
                    finish(data, active, 0, timestamp_ns);
                    return;
                };
                if active {
                    let clock = ClockInfo {
                        position: data.position,
                        timestamp_ns,
                        frames,
                    };
                    let mut io = StreamIo {
                        input: None,
                        output: Some(samples),
                    };
                    (data.callback)(&mut io, &clock);
                } else {
                    samples.fill(0.0);
                }
                frames
            };
            let chunk = plane.chunk_mut();
            *chunk.offset_mut() = 0;
            *chunk.stride_mut() = stride as i32;
            *chunk.size_mut() = (frames * stride) as u32;
            frames
        }
        DeviceDirection::Capture => {
            let (offset, size) = {
                let chunk = plane.chunk();
                (chunk.offset() as usize, chunk.size() as usize)
            };
            let Some(bytes) = plane.data() else {
                finish(data, active, 0, timestamp_ns);
                return;
            };
            let end = offset.saturating_add(size).min(bytes.len());
            let start = offset.min(end);
            let frames = (end - start) / stride;
            let Ok(samples) =
                bytemuck::try_cast_slice::<u8, f32>(&bytes[start..start + frames * stride])
            else {
                finish(data, active, 0, timestamp_ns);
                return;
            };
            if active {
                let clock = ClockInfo {
                    position: data.position,
                    timestamp_ns,
                    frames,
                };
                let mut io = StreamIo {
                    input: Some(samples),
                    output: None,
                };
                (data.callback)(&mut io, &clock);
            }
            frames
        }
    };

    finish(data, active, frames, timestamp_ns);
}

/// Publie l'horloge et rend la main à un `stop` en attente.
fn finish(data: &mut ProcessData, active: bool, frames: usize, timestamp_ns: u64) {
    if active {
        data.position += frames as u64;
        data.shared.position.store(data.position, Ordering::Release);
        data.shared.frames.store(frames, Ordering::Release);
        data.shared
            .timestamp_ns
            .store(timestamp_ns, Ordering::Release);
        data.shared.state.store(STATE_ACTIVE, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use conduit_core::types::SampleRate;

    #[test]
    fn state_machine_starts_stopped_and_toggles() {
        let shared = StreamShared::default();
        assert!(!shared.is_running());
        shared.activate();
        assert!(shared.is_running());
        shared.activate();
        assert!(shared.is_running());
        shared.deactivate();
        assert!(!shared.is_running());
        shared.deactivate();
        assert!(!shared.is_running());
    }

    #[test]
    fn clock_reads_back_what_the_callback_published() {
        let shared = Arc::new(StreamShared::default());
        assert_eq!(shared.clock(), ClockInfo::default());
        let mut data = ProcessData {
            callback: Box::new(|_, _| {}),
            shared: Arc::clone(&shared),
            direction: DeviceDirection::Render,
            channels: 2,
            block_frames: 128,
            position: 0,
            last_timestamp_ns: 0,
            epoch: Instant::now(),
        };
        shared.activate();
        finish(&mut data, true, 128, 42);
        finish(&mut data, true, 128, 43);
        let clock = shared.clock();
        assert_eq!(clock.position, 256);
        assert_eq!(clock.frames, 128);
        assert_eq!(clock.timestamp_ns, 43);
        // Un rappel inactif ne bouge pas l'horloge.
        finish(&mut data, false, 128, 99);
        assert_eq!(shared.clock().position, 256);
    }

    #[test]
    fn disconnect_flag() {
        let shared = StreamShared::default();
        assert!(!shared.is_disconnected());
        shared.mark_disconnected();
        assert!(shared.is_disconnected());
    }

    #[test]
    fn format_pod_is_serialisable_and_rejects_nothing_reasonable() {
        let id = DeviceId::new("x");
        for rate in [SampleRate::HZ_44100, SampleRate::HZ_48000] {
            for channels in [1usize, 2, 8] {
                let pod = format_pod(
                    &id,
                    StreamFormat {
                        sample_rate: rate,
                        channels,
                        block_frames: 256,
                    },
                )
                .expect("pod");
                assert!(!pod.is_empty());
                assert!(spa::pod::Pod::from_bytes(&pod).is_some());
            }
        }
    }
}
