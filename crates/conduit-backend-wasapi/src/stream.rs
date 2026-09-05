//! Le fil d'un flux WASAPI et la poignée [`WasapiHandle`].
//!
//! Chaque flux a **son fil**, créé par [`DeviceHandle::start`] et joint par
//! [`DeviceHandle::stop`]. Ce fil initialise COM en MTA, se promeut en temps réel
//! (`conduit_backend::rt`, résultat mémorisé, jamais bloquant), résout les
//! références agiles vers `IAudioClient` et le service de rendu ou de capture, puis
//! boucle sur `WaitForMultipleObjects(arrêt, tampon, 2 s)` :
//!
//! - **rendu** : `GetCurrentPadding` → trames libres = tampon − padding →
//!   `GetBuffer(n)` → le rappel écrit directement dans le tampon WASAPI vu comme
//!   `&mut [f32]` (`bytemuck::try_cast_slice_mut` ; si l'alignement n'était pas
//!   celui de `f32`, on passe par un tampon intermédiaire pré-alloué) →
//!   `ReleaseBuffer(n, 0)` ;
//! - **capture** : tant que `GetNextPacketSize` > 0 : `GetBuffer` → rappel avec
//!   `input = &[f32]` (un tampon de zéros pré-alloué si `AUDCLNT_BUFFERFLAGS_SILENT`)
//!   → `ReleaseBuffer`.
//!
//! Une fois la boucle lancée, **aucune allocation ni verrou** (dev-guide §2) : les
//! tampons intermédiaires sont alloués à l'ouverture, l'horloge est publiée par
//! atomiques. `AUDCLNT_E_DEVICE_INVALIDATED` (ou `RESOURCES_INVALIDATED`) fait
//! sortir de la boucle : le flux est marqué **déconnecté** (`is_running()` devient
//! faux, `stop()` renvoie `Disconnected`, `start()` le refuse), sans panique.
//!
//! `stop()` signale l'événement d'arrêt, joint le fil (qui a fait `Stop` puis
//! `Reset` avant de rendre la main) : aucun rappel n'est en cours au retour. `Drop`
//! appelle `stop()`.
//!
//! `ClockInfo` (M1b-33 branchera `IAudioClock`) : `position` = trames livrées au
//! rappel depuis `start()`, `timestamp_ns` = `Instant` depuis l'`epoch` de
//! l'ouverture, `frames` = trames de ce rappel (un paquet en capture).

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use conduit_backend::rt::{promote_current_thread, RtOutcome};
use conduit_backend::{
    AudioCallback, BackendError, ClockInfo, DeviceHandle, DeviceId, DeviceInfo, StreamFormat,
    StreamIo,
};
use windows::Win32::Media::Audio::{
    IAudioCaptureClient, IAudioClient, IAudioRenderClient, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_RESOURCES_INVALIDATED,
};

use crate::com::{platform_error, ComApartment, Event, Woken};
use crate::open::{Opened, Service, StreamLatency, StreamObjects};

/// Délai maximal entre deux réveils avant d'aller « toucher » le client : un
/// périphérique disparu sans signaler son événement est ainsi détecté.
const WAKE_TIMEOUT: Duration = Duration::from_secs(2);

/// Octets d'un échantillon `f32`.
const SAMPLE_BYTES: usize = core::mem::size_of::<f32>();

/// État partagé entre la poignée et le fil du flux.
#[derive(Debug, Default)]
pub(crate) struct StreamShared {
    /// Rappels actifs : vrai de `start()` à la fin du fil.
    running: AtomicBool,
    /// Le périphérique a été invalidé : plus aucun démarrage possible.
    disconnected: AtomicBool,
    /// Trames livrées depuis `start()`, au début du prochain rappel.
    position: AtomicU64,
    /// Horodatage du dernier rappel, nanosecondes depuis l'ouverture.
    timestamp_ns: AtomicU64,
    /// Trames du dernier rappel.
    frames: AtomicUsize,
    /// Résultat de la promotion temps réel du fil (écrit une fois, avant la boucle).
    rt: Mutex<Option<RtOutcome>>,
}

impl StreamShared {
    fn clock(&self) -> ClockInfo {
        ClockInfo {
            position: self.position.load(Ordering::Acquire),
            timestamp_ns: self.timestamp_ns.load(Ordering::Acquire),
            frames: self.frames.load(Ordering::Acquire),
        }
    }
}

/// Tout ce que le fil du flux possède pendant qu'il tourne, et qui revient à la
/// poignée quand il s'arrête (pour le prochain `start()`).
struct Worker {
    objects: StreamObjects,
    callback: AudioCallback,
    shared: Arc<StreamShared>,
    stop: Arc<Event>,
    device: DeviceId,
    channels: usize,
    /// Trames du tampon WASAPI : borne des trames d'un réveil.
    buffer_frames: usize,
    /// Tampon intermédiaire (`buffer_frames × channels`), pré-alloué : repli
    /// d'alignement en rendu, silence ou repli d'alignement en capture.
    scratch: Vec<f32>,
    /// Base des horodatages, fixée à l'ouverture.
    epoch: Instant,
    /// Position courante, propriété exclusive du fil du flux.
    position: u64,
}

/// Ce que le fil rend en s'arrêtant.
struct Finished {
    worker: Worker,
    outcome: Result<(), BackendError>,
}

/// Où en est le fil du flux.
enum Thread {
    /// Aucun fil : les objets attendent le prochain `start()`.
    Idle(Box<Worker>),
    /// Fil lancé (peut-être déjà terminé de lui-même : `stop()` le joindra).
    Running(JoinHandle<Finished>),
    /// Fil perdu (panique du rappel, échec de lancement) : le flux est inutilisable.
    Lost,
}

/// Poignée d'un flux WASAPI ouvert en mode partagé.
///
/// `Send` : elle ne détient que des références agiles, des atomiques, un
/// événement et, entre deux démarrages, le rappel. Elle vit indépendamment du
/// [`WasapiBackend`](crate::WasapiBackend) qui l'a créée.
pub struct WasapiHandle {
    info: DeviceInfo,
    format: StreamFormat,
    shared: Arc<StreamShared>,
    stop: Arc<Event>,
    latency: StreamLatency,
    thread: Thread,
    /// Dernière erreur d'un fil perdu, rendue par `start()`.
    lost_reason: Option<BackendError>,
}

impl WasapiHandle {
    /// Assemble la poignée à partir de ce que le fil MMDevice a ouvert.
    pub(crate) fn new(opened: Opened, callback: AudioCallback) -> Result<Self, BackendError> {
        let Opened {
            info,
            format,
            objects,
        } = opened;
        let shared = Arc::new(StreamShared::default());
        let stop = Arc::new(Event::new(true)?);
        let buffer_frames = objects.latency.buffer_frames.max(1);
        let channels = format.channels.max(1);
        let worker = Worker {
            device: info.id.clone(),
            objects,
            callback,
            shared: Arc::clone(&shared),
            stop: Arc::clone(&stop),
            channels,
            buffer_frames,
            scratch: vec![0.0; buffer_frames * channels],
            epoch: Instant::now(),
            position: 0,
        };
        Ok(Self {
            latency: worker.objects.latency,
            info,
            format,
            shared,
            stop,
            thread: Thread::Idle(Box::new(worker)),
            lost_reason: None,
        })
    }

    /// Latence et tailles de tampon telles que WASAPI les rapporte à l'ouverture
    /// (hors trait `DeviceHandle`) : tampon partagé, période effective,
    /// `GetStreamLatency`, chemin d'initialisation.
    pub fn latency(&self) -> StreamLatency {
        self.latency
    }

    /// Résultat de la promotion temps réel du fil du flux, connu dès que le fil
    /// démarre ; `None` avant le premier `start()`.
    pub fn rt_outcome(&self) -> Option<RtOutcome> {
        self.shared.rt.lock().ok().and_then(|rt| rt.clone())
    }

    fn disconnected_error(&self) -> BackendError {
        BackendError::Disconnected(self.info.id.clone())
    }
}

impl core::fmt::Debug for WasapiHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WasapiHandle")
            .field("device", &self.info.id)
            .field("format", &self.format)
            .field("latency", &self.latency)
            .field("running", &self.is_running())
            .field(
                "disconnected",
                &self.shared.disconnected.load(Ordering::Acquire),
            )
            .finish()
    }
}

impl DeviceHandle for WasapiHandle {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn format(&self) -> StreamFormat {
        self.format
    }

    /// Lance le fil du flux. Idempotent tant que le fil tourne.
    fn start(&mut self) -> Result<(), BackendError> {
        if self.shared.disconnected.load(Ordering::Acquire) {
            return Err(self.disconnected_error());
        }
        let worker = match core::mem::replace(&mut self.thread, Thread::Lost) {
            Thread::Idle(worker) => worker,
            Thread::Running(join) => {
                if join.is_finished() {
                    // Le fil s'est arrêté seul (périphérique invalidé, erreur) :
                    // on récolte son verdict avant de relancer.
                    self.thread = Thread::Running(join);
                    self.stop()?;
                    return self.start();
                }
                self.thread = Thread::Running(join);
                return Ok(());
            }
            Thread::Lost => {
                return Err(self.lost_reason.clone().unwrap_or_else(|| {
                    BackendError::Platform(format!(
                        "le fil du flux {} est perdu : rouvrez le périphérique",
                        self.info.id
                    ))
                }));
            }
        };
        self.stop.reset();
        self.shared.running.store(true, Ordering::Release);
        let name = format!("conduit-wasapi-{}", self.info.direction);
        match std::thread::Builder::new()
            .name(name)
            .spawn(move || run(*worker))
        {
            Ok(join) => {
                self.thread = Thread::Running(join);
                Ok(())
            }
            Err(e) => {
                self.shared.running.store(false, Ordering::Release);
                let reason = BackendError::Platform(format!("fil du flux non démarré : {e}"));
                self.lost_reason = Some(reason.clone());
                Err(reason)
            }
        }
    }

    /// Arrête le fil du flux et le joint : aucun rappel en cours au retour. Rend
    /// l'erreur qui a fait sortir le fil de lui-même, le cas échéant.
    fn stop(&mut self) -> Result<(), BackendError> {
        let join = match core::mem::replace(&mut self.thread, Thread::Lost) {
            Thread::Running(join) => join,
            other => {
                self.thread = other;
                return Ok(());
            }
        };
        self.stop.set();
        match join.join() {
            Ok(Finished { worker, outcome }) => {
                self.thread = Thread::Idle(Box::new(worker));
                outcome
            }
            Err(_) => {
                let reason = BackendError::Platform(format!(
                    "le fil du flux {} a paniqué : rouvrez le périphérique",
                    self.info.id
                ));
                self.lost_reason = Some(reason.clone());
                Err(reason)
            }
        }
    }

    fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Acquire)
    }

    fn clock(&self) -> ClockInfo {
        self.shared.clock()
    }
}

impl Drop for WasapiHandle {
    fn drop(&mut self) {
        let _ = self.stop();
        // Les objets WASAPI sont libérés ici, sur un fil qui n'a peut-être jamais
        // initialisé COM (fil de test, fil de gestion du démon) : on lui donne un
        // appartement le temps de la libération. Si l'appel échoue (fil déjà en
        // STA, par exemple), COM y est initialisé de toute façon et la libération
        // reste valide.
        if let Thread::Idle(worker) = core::mem::replace(&mut self.thread, Thread::Lost) {
            let apartment = ComApartment::initialize_mta();
            drop(worker);
            drop(apartment);
        }
    }
}

/// Remet `running` à faux quand le fil se termine, panique comprise.
struct RunningGuard(Arc<StreamShared>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        self.0.running.store(false, Ordering::Release);
    }
}

/// Corps du fil du flux : rend le `Worker` pour le prochain démarrage.
fn run(mut worker: Worker) -> Finished {
    let _guard = RunningGuard(Arc::clone(&worker.shared));
    let outcome = run_inner(&mut worker);
    if let Err(BackendError::Disconnected(_)) = &outcome {
        worker.shared.disconnected.store(true, Ordering::Release);
    }
    Finished { worker, outcome }
}

fn run_inner(worker: &mut Worker) -> Result<(), BackendError> {
    // Déclaré en premier : détruit en dernier, après les interfaces résolues.
    let _apartment = ComApartment::initialize_mta()?;
    if let Ok(mut rt) = worker.shared.rt.lock() {
        *rt = Some(promote_current_thread());
    }
    let client = worker
        .objects
        .client
        .resolve()
        .map_err(|e| platform_error("AgileReference::Resolve(IAudioClient)", &e))?;
    let service = match &worker.objects.service {
        Service::Render(render) => Resolved::Render(
            render
                .resolve()
                .map_err(|e| platform_error("AgileReference::Resolve(IAudioRenderClient)", &e))?,
        ),
        Service::Capture(capture) => Resolved::Capture(
            capture
                .resolve()
                .map_err(|e| platform_error("AgileReference::Resolve(IAudioCaptureClient)", &e))?,
        ),
    };

    worker.position = 0;
    worker.shared.position.store(0, Ordering::Release);
    worker.shared.frames.store(0, Ordering::Release);

    // Un tampon de silence avant `Start` : le moteur ne lit jamais de données
    // indéterminées, et le premier réveil arrive une période plus tard.
    if let Resolved::Render(render) = &service {
        prefill_silence(&client, render, worker.buffer_frames)
            .map_err(|e| classify(worker, "IAudioRenderClient::GetBuffer (préremplissage)", &e))?;
    }
    // SAFETY: client initialisé, événement enregistré.
    unsafe { client.Start() }.map_err(|e| classify(worker, "IAudioClient::Start", &e))?;

    let outcome = audio_loop(worker, &client, &service);

    // Ignorés : après une invalidation ils échouent aussi, et il n'y a rien de
    // mieux à faire.
    // SAFETY: client démarré (ou invalidé : l'appel échoue proprement).
    let _ = unsafe { client.Stop() };
    // SAFETY: client arrêté.
    let _ = unsafe { client.Reset() };
    outcome
}

/// Interfaces résolues sur le fil du flux.
enum Resolved {
    Render(IAudioRenderClient),
    Capture(IAudioCaptureClient),
}

/// Boucle événementielle : **aucune allocation ni verrou** ici ni dans ce qu'elle
/// appelle.
fn audio_loop(
    worker: &mut Worker,
    client: &IAudioClient,
    service: &Resolved,
) -> Result<(), BackendError> {
    loop {
        let woken = Event::wait_either(&worker.stop, &worker.objects.event, WAKE_TIMEOUT)?;
        match woken {
            Woken::First => return Ok(()),
            // Réveil normal, ou délai écoulé : dans ce dernier cas, le passage par
            // `GetCurrentPadding` / `GetNextPacketSize` révèle un périphérique
            // invalidé qui n'aurait plus signalé son événement.
            Woken::Second | Woken::Timeout => {}
        }
        let result = match service {
            Resolved::Render(render) => render_once(worker, client, render),
            Resolved::Capture(capture) => capture_all(worker, capture),
        };
        if let Err((call, e)) = result {
            return Err(classify(worker, call, &e));
        }
    }
}

/// Un réveil en rendu : remplir tout l'espace libre du tampon.
fn render_once(
    worker: &mut Worker,
    client: &IAudioClient,
    render: &IAudioRenderClient,
) -> Result<(), (&'static str, windows::core::Error)> {
    // SAFETY: client démarré.
    let padding = unsafe { client.GetCurrentPadding() }
        .map_err(|e| ("IAudioClient::GetCurrentPadding", e))?;
    let frames = worker
        .buffer_frames
        .saturating_sub(padding as usize)
        .min(worker.buffer_frames);
    if frames == 0 {
        return Ok(());
    }
    // SAFETY: `frames ≤ tampon − padding`, condition de `GetBuffer`.
    let ptr = unsafe { render.GetBuffer(frames as u32) }
        .map_err(|e| ("IAudioRenderClient::GetBuffer", e))?;
    let samples = frames * worker.channels;
    let timestamp_ns = elapsed_ns(worker.epoch);
    let clock = ClockInfo {
        position: worker.position,
        timestamp_ns,
        frames,
    };
    // SAFETY: `GetBuffer(frames)` a réussi : `ptr` pointe `frames × nBlockAlign`
    // octets (`nBlockAlign = channels × 4` pour le format float32 négocié), à nous
    // jusqu'à `ReleaseBuffer`. Aucun autre accès pendant ce temps.
    let bytes = unsafe { core::slice::from_raw_parts_mut(ptr, samples * SAMPLE_BYTES) };
    match bytemuck::try_cast_slice_mut::<u8, f32>(bytes) {
        Ok(output) => {
            let mut io = StreamIo {
                input: None,
                output: Some(output),
            };
            (worker.callback)(&mut io, &clock);
        }
        Err(_) => {
            // Tampon non aligné sur 4 octets (jamais vu avec WASAPI, mais rien ne
            // l'interdit) : rendu dans le tampon intermédiaire, puis copie.
            let scratch = &mut worker.scratch[..samples];
            let mut io = StreamIo {
                input: None,
                output: Some(scratch),
            };
            (worker.callback)(&mut io, &clock);
            bytes.copy_from_slice(bytemuck::cast_slice(&worker.scratch[..samples]));
        }
    }
    // SAFETY: apparié au `GetBuffer` réussi ci-dessus, même nombre de trames.
    unsafe { render.ReleaseBuffer(frames as u32, 0) }
        .map_err(|e| ("IAudioRenderClient::ReleaseBuffer", e))?;
    publish(worker, frames, timestamp_ns);
    Ok(())
}

/// Un réveil en capture : livrer tous les paquets disponibles, un rappel chacun.
fn capture_all(
    worker: &mut Worker,
    capture: &IAudioCaptureClient,
) -> Result<(), (&'static str, windows::core::Error)> {
    loop {
        // SAFETY: client démarré.
        let packet = unsafe { capture.GetNextPacketSize() }
            .map_err(|e| ("IAudioCaptureClient::GetNextPacketSize", e))?;
        if packet == 0 {
            return Ok(());
        }
        let mut ptr: *mut u8 = core::ptr::null_mut();
        let mut packet_frames = 0u32;
        let mut flags = 0u32;
        // SAFETY: les trois sorties sont des locales vivantes ; positions ignorées.
        unsafe { capture.GetBuffer(&mut ptr, &mut packet_frames, &mut flags, None, None) }
            .map_err(|e| ("IAudioCaptureClient::GetBuffer", e))?;
        // Un paquet ne dépasse jamais le tampon ; la borne protège `scratch`.
        let frames = (packet_frames as usize).min(worker.buffer_frames);
        let samples = frames * worker.channels;
        let timestamp_ns = elapsed_ns(worker.epoch);
        if frames > 0 {
            let clock = ClockInfo {
                position: worker.position,
                timestamp_ns,
                frames,
            };
            let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
            let Worker {
                callback, scratch, ..
            } = worker;
            let input: &[f32] = if silent || ptr.is_null() {
                scratch[..samples].fill(0.0);
                &scratch[..samples]
            } else {
                // SAFETY: `GetBuffer` a réussi avec `packet_frames` trames à `ptr`,
                // lisibles jusqu'à `ReleaseBuffer` ; `frames ≤ packet_frames`.
                let bytes = unsafe { core::slice::from_raw_parts(ptr, samples * SAMPLE_BYTES) };
                match bytemuck::try_cast_slice::<u8, f32>(bytes) {
                    Ok(input) => input,
                    Err(_) => {
                        bytemuck::cast_slice_mut::<f32, u8>(&mut scratch[..samples])
                            .copy_from_slice(bytes);
                        &scratch[..samples]
                    }
                }
            };
            let mut io = StreamIo {
                input: Some(input),
                output: None,
            };
            (callback)(&mut io, &clock);
        }
        // SAFETY: apparié au `GetBuffer` réussi, avec le nombre de trames rendu.
        unsafe { capture.ReleaseBuffer(packet_frames) }
            .map_err(|e| ("IAudioCaptureClient::ReleaseBuffer", e))?;
        publish(worker, frames, timestamp_ns);
    }
}

/// Remplit l'espace libre du tampon de rendu de silence, avant `Start`.
fn prefill_silence(
    client: &IAudioClient,
    render: &IAudioRenderClient,
    buffer_frames: usize,
) -> Result<(), windows::core::Error> {
    // SAFETY: client initialisé.
    let padding = unsafe { client.GetCurrentPadding() }?;
    let frames = buffer_frames.saturating_sub(padding as usize);
    if frames == 0 {
        return Ok(());
    }
    // SAFETY: `frames ≤ tampon − padding`.
    unsafe { render.GetBuffer(frames as u32) }?;
    // SAFETY: apparié au `GetBuffer` réussi ; `SILENT` dispense d'écrire.
    unsafe { render.ReleaseBuffer(frames as u32, AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) }
}

/// Publie l'horloge après un rappel.
fn publish(worker: &mut Worker, frames: usize, timestamp_ns: u64) {
    worker.position += frames as u64;
    worker
        .shared
        .position
        .store(worker.position, Ordering::Release);
    worker.shared.frames.store(frames, Ordering::Release);
    worker
        .shared
        .timestamp_ns
        .store(timestamp_ns, Ordering::Release);
}

/// Nanosecondes écoulées depuis l'ouverture (monotone : `Instant`).
fn elapsed_ns(epoch: Instant) -> u64 {
    u64::try_from(epoch.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Une invalidation du périphérique devient [`BackendError::Disconnected`], le
/// reste [`BackendError::Platform`]. Hors boucle temps réel (le fil s'arrête).
fn classify(worker: &Worker, call: &str, error: &windows::core::Error) -> BackendError {
    if error.code() == AUDCLNT_E_DEVICE_INVALIDATED
        || error.code() == AUDCLNT_E_RESOURCES_INVALIDATED
    {
        BackendError::Disconnected(worker.device.clone())
    } else {
        platform_error(call, error)
    }
}
