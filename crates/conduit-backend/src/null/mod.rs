//! Backend `null` : périphériques et câbles simulés à horloge virtuelle.
//!
//! - **Mode manuel** : le test fait avancer le temps avec [`NullBackend::advance`] ;
//!   les rappels sont exécutés dans l'ordre chronologique, de façon déterministe.
//! - **Mode timer** : un fil cadence les rappels en temps réel
//!   ([`NullBackend::start_timer`]).
//!
//! Chaque périphérique a sa dérive et sa gigue. Les périphériques de capture
//! produisent un [`Signal`] injecté ; les périphériques de rendu enregistrent ce
//! qu'ils reçoivent. Les câbles sont des paires rendu/capture en boucle locale.

mod device;

pub use device::{NullDevice, NullDeviceSpec, Signal, SignalFn};

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use conduit_core::types::{ChannelCount, SampleRate};

use crate::cable::{
    validate_cable_name, CableControl, CableError, CableFormat, CableId, CableInfo, CableSpec,
};
use crate::device::{
    AudioCallback, Backend, BackendError, ClockInfo, DeviceDirection, DeviceHandle, DeviceId,
    DeviceInfo, StreamFormat,
};
use crate::event::{DeviceEvent, EventBroadcaster, EventReceiver};

use device::{Loopback, Stream};

struct Inner {
    devices: BTreeMap<DeviceId, Arc<NullDevice>>,
    defaults: [Option<DeviceId>; 2],
    events: EventBroadcaster,
    cables: BTreeMap<CableId, CableInfo>,
    next_cable: u32,
    max_cables: usize,
}

/// Backend simulé.
pub struct NullBackend {
    inner: Arc<Mutex<Inner>>,
    /// Temps virtuel en nanosecondes (mode manuel : avancé par `advance`).
    clock: Arc<AtomicU64>,
    timer: Option<TimerThread>,
}

struct TimerThread {
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}

impl core::fmt::Debug for NullBackend {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let inner = self.inner.lock().unwrap();
        f.debug_struct("NullBackend")
            .field("devices", &inner.devices.len())
            .field("cables", &inner.cables.len())
            .field("now_ns", &self.clock.load(Ordering::Relaxed))
            .field("timer", &self.timer.is_some())
            .finish()
    }
}

impl Default for NullBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl NullBackend {
    /// Limite de câbles par défaut (comme macOS et Linux dans SPEC F-06).
    pub const DEFAULT_MAX_CABLES: usize = 32;

    /// Backend vide, mode manuel.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                devices: BTreeMap::new(),
                defaults: [None, None],
                events: EventBroadcaster::default(),
                cables: BTreeMap::new(),
                next_cable: 1,
                max_cables: Self::DEFAULT_MAX_CABLES,
            })),
            clock: Arc::new(AtomicU64::new(0)),
            timer: None,
        }
    }

    /// Change la limite de câbles.
    pub fn with_max_cables(self, max: usize) -> Self {
        self.inner.lock().unwrap().max_cables = max;
        self
    }

    /// Ajoute un périphérique (émet `Added`). Retourne son identifiant.
    pub fn add_device(&self, spec: NullDeviceSpec) -> DeviceId {
        let mut inner = self.inner.lock().unwrap();
        let id = DeviceId::new(format!(
            "null:{}",
            spec.name.to_lowercase().replace(' ', "-")
        ));
        let id = Self::unique_id(&inner, id);
        let dev = Arc::new(NullDevice::new(
            id.clone(),
            spec,
            None,
            None,
            Arc::clone(&self.clock),
        ));
        let info = dev.info();
        inner.devices.insert(id.clone(), dev);
        let dir = info.direction;
        inner.events.send(DeviceEvent::Added(info));
        if inner.defaults[dir_index(dir)].is_none() {
            inner.defaults[dir_index(dir)] = Some(id.clone());
            inner.events.send(DeviceEvent::DefaultChanged {
                direction: dir,
                id: Some(id.clone()),
            });
        }
        id
    }

    fn unique_id(inner: &Inner, base: DeviceId) -> DeviceId {
        if !inner.devices.contains_key(&base) {
            return base;
        }
        let mut n = 2;
        loop {
            let candidate = DeviceId::new(format!("{base}#{n}"));
            if !inner.devices.contains_key(&candidate) {
                return candidate;
            }
            n += 1;
        }
    }

    /// Retire un périphérique (émet `Removed`) : ses flux s'arrêtent, son handle
    /// signale la déconnexion. Retourne `false` s'il n'existait pas.
    pub fn remove_device(&self, id: &DeviceId) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(dev) = inner.devices.remove(id) else {
            return false;
        };
        dev.disconnect();
        let dir = dev.info().direction;
        inner.events.send(DeviceEvent::Removed { id: id.clone() });
        if inner.defaults[dir_index(dir)].as_ref() == Some(id) {
            let next = inner
                .devices
                .values()
                .find(|d| d.info().direction == dir)
                .map(|d| d.id().clone());
            inner.defaults[dir_index(dir)] = next.clone();
            inner.events.send(DeviceEvent::DefaultChanged {
                direction: dir,
                id: next,
            });
        }
        true
    }

    /// Fixe le périphérique par défaut d'un sens (émet `DefaultChanged`).
    pub fn set_default(
        &self,
        direction: DeviceDirection,
        id: Option<DeviceId>,
    ) -> Result<(), BackendError> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(id) = &id {
            let dev = inner
                .devices
                .get(id)
                .ok_or_else(|| BackendError::NotFound(id.clone()))?;
            if dev.info().direction != direction {
                return Err(BackendError::UnsupportedFormat {
                    device: id.clone(),
                    reason: format!("ce périphérique est en {}", dev.info().direction),
                });
            }
        }
        inner.defaults[dir_index(direction)] = id.clone();
        inner
            .events
            .send(DeviceEvent::DefaultChanged { direction, id });
        Ok(())
    }

    /// Accès à un périphérique simulé (injection de signal, enregistrement).
    pub fn device(&self, id: &DeviceId) -> Option<Arc<NullDevice>> {
        self.inner.lock().unwrap().devices.get(id).cloned()
    }

    /// Temps virtuel courant (mode manuel), en nanosecondes.
    pub fn now_ns(&self) -> u64 {
        self.clock.load(Ordering::Relaxed)
    }

    /// Fait avancer le temps virtuel de `dt` et exécute, dans l'ordre chronologique,
    /// tous les rappels dus. Retourne le nombre de rappels exécutés.
    pub fn advance(&self, dt: Duration) -> usize {
        let dt_ns = dt.as_nanos() as u64;
        let deadline = self.clock.fetch_add(dt_ns, Ordering::Relaxed) + dt_ns;
        self.run_due(deadline)
    }

    /// Exécute les rappels dus jusqu'à `deadline_ns`. Le verrou interne n'est pas
    /// tenu pendant un rappel.
    fn run_due(&self, deadline_ns: u64) -> usize {
        let mut count = 0;
        loop {
            let next = {
                let inner = self.inner.lock().unwrap();
                inner
                    .devices
                    .values()
                    .filter_map(|d| d.stream().map(|s| (s, Arc::clone(d))))
                    .filter_map(|(s, d)| {
                        let st = s.lock().unwrap();
                        st.running.then_some((st.next_tick_ns, Arc::clone(&s), d))
                    })
                    .min_by_key(|(t, _, _)| *t)
            };
            match next {
                Some((tick, stream, dev)) if tick <= deadline_ns => {
                    dev.run_callback(&stream, tick);
                    count += 1;
                }
                _ => return count,
            }
        }
    }

    /// Démarre le fil timer : les rappels sont cadencés en temps réel à partir de
    /// maintenant (le temps virtuel suit l'horloge monotone). Sans effet si déjà actif.
    pub fn start_timer(&mut self) {
        if self.timer.is_some() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let inner = Arc::clone(&self.inner);
        let clock = Arc::clone(&self.clock);
        let stop2 = Arc::clone(&stop);
        let base = self.now_ns();
        let start = Instant::now();
        let handle = std::thread::Builder::new()
            .name("conduit-null-timer".into())
            .spawn(move || {
                let me = NullBackend {
                    inner,
                    clock,
                    timer: None,
                };
                while !stop2.load(Ordering::Relaxed) {
                    let now = base + start.elapsed().as_nanos() as u64;
                    me.clock.store(now, Ordering::Relaxed);
                    me.run_due(now);
                    std::thread::sleep(Duration::from_micros(500));
                }
            })
            .expect("fil timer");
        self.timer = Some(TimerThread { stop, handle });
    }

    /// Arrête le fil timer (attend sa fin).
    pub fn stop_timer(&mut self) {
        if let Some(t) = self.timer.take() {
            t.stop.store(true, Ordering::Relaxed);
            let _ = t.handle.join();
        }
    }

    /// Vrai si le fil timer tourne.
    pub fn timer_running(&self) -> bool {
        self.timer.is_some()
    }

    fn cable_devices(
        id: CableId,
        name: &str,
        channels: ChannelCount,
        rate: SampleRate,
        block: usize,
    ) -> (NullDeviceSpec, NullDeviceSpec) {
        let render = NullDeviceSpec {
            name: name.to_string(),
            direction: DeviceDirection::Render,
            channels: channels.as_usize(),
            sample_rate: rate,
            block,
            ..NullDeviceSpec::default()
        };
        let capture = NullDeviceSpec {
            direction: DeviceDirection::Capture,
            ..render.clone()
        };
        let _ = id;
        (render, capture)
    }
}

impl Drop for NullBackend {
    fn drop(&mut self) {
        self.stop_timer();
    }
}

impl Clone for NullBackend {
    /// Une poignée partageant les mêmes périphériques, câbles et horloge (le fil
    /// timer n'est pas partagé). Permet à un test de scripter le backend que le moteur
    /// possède.
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            clock: Arc::clone(&self.clock),
            timer: None,
        }
    }
}

fn dir_index(d: DeviceDirection) -> usize {
    match d {
        DeviceDirection::Capture => 0,
        DeviceDirection::Render => 1,
    }
}

impl Backend for NullBackend {
    fn name(&self) -> &'static str {
        "null"
    }

    fn devices(&self) -> Result<Vec<DeviceInfo>, BackendError> {
        let inner = self.inner.lock().unwrap();
        Ok(inner
            .devices
            .values()
            .map(|d| {
                let mut info = d.info();
                info.is_default =
                    inner.defaults[dir_index(info.direction)].as_ref() == Some(d.id());
                info
            })
            .collect())
    }

    fn default_device(&self, direction: DeviceDirection) -> Option<DeviceId> {
        self.inner.lock().unwrap().defaults[dir_index(direction)].clone()
    }

    fn open(
        &mut self,
        id: &DeviceId,
        format: StreamFormat,
        callback: AudioCallback,
    ) -> Result<Box<dyn DeviceHandle>, BackendError> {
        let dev = {
            let inner = self.inner.lock().unwrap();
            inner
                .devices
                .get(id)
                .cloned()
                .ok_or_else(|| BackendError::NotFound(id.clone()))?
        };
        let stream = dev.open(format, callback)?;
        let format = stream.lock().unwrap().format;
        let info = dev.info();
        Ok(Box::new(NullHandle {
            dev,
            stream,
            format,
            info,
        }))
    }

    fn subscribe(&mut self) -> EventReceiver {
        self.inner.lock().unwrap().events.subscribe()
    }

    fn cable_control(&mut self) -> Option<&mut dyn CableControl> {
        Some(self)
    }
}

/// Handle d'un flux simulé.
pub struct NullHandle {
    dev: Arc<NullDevice>,
    stream: Arc<Mutex<Stream>>,
    format: StreamFormat,
    info: DeviceInfo,
}

impl core::fmt::Debug for NullHandle {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NullHandle")
            .field("device", self.dev.id())
            .field("format", &self.format)
            .finish()
    }
}

impl DeviceHandle for NullHandle {
    fn info(&self) -> &DeviceInfo {
        &self.info
    }

    fn format(&self) -> StreamFormat {
        self.format
    }

    fn start(&mut self) -> Result<(), BackendError> {
        if !self.dev.is_present() {
            return Err(BackendError::Disconnected(self.dev.id().clone()));
        }
        let mut s = self.stream.lock().unwrap();
        if !s.running {
            // Premier rappel dû maintenant, puis cadence nominale.
            s.next_tick_ns = self.dev.now_ns();
            s.running = true;
        }
        Ok(())
    }

    fn stop(&mut self) -> Result<(), BackendError> {
        self.stream.lock().unwrap().running = false;
        Ok(())
    }

    fn is_running(&self) -> bool {
        self.dev.is_present() && self.stream.lock().unwrap().running
    }

    fn clock(&self) -> ClockInfo {
        let s = self.stream.lock().unwrap();
        ClockInfo {
            position: s.position,
            timestamp_ns: s.last_tick_ns,
            frames: s.format.block_frames,
        }
    }
}

impl Drop for NullHandle {
    fn drop(&mut self) {
        self.stream.lock().unwrap().running = false;
        self.dev.close();
    }
}

impl CableControl for NullBackend {
    fn max_cables(&self) -> usize {
        self.inner.lock().unwrap().max_cables
    }

    fn list(&self) -> Result<Vec<CableInfo>, CableError> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .cables
            .values()
            .cloned()
            .collect())
    }

    fn create(&mut self, spec: CableSpec) -> Result<CableInfo, CableError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.cables.len() >= inner.max_cables {
            return Err(CableError::LimitReached {
                max: inner.max_cables,
            });
        }
        let id = CableId(inner.next_cable);
        let name = spec.name.unwrap_or_else(|| id.to_string());
        validate_cable_name(&name)?;
        inner.next_cable += 1;
        // `spec.channels` est ignoré dès que `spec.format` est donné : le format porte
        // déjà ses canaux, et laisser les deux se contredire n'apporterait qu'un piège.
        let format = spec.format.unwrap_or(CableFormat {
            channels: spec.channels,
            ..CableFormat::default()
        });
        let (render_spec, capture_spec) =
            Self::cable_devices(id, &name, format.channels, format.sample_rate, 480);
        let loopback = Arc::new(Loopback::new(format.channels.as_usize(), 480 * 4));
        let render_id = DeviceId::new(format!("null:cable{}:render", id.0));
        let capture_id = DeviceId::new(format!("null:cable{}:capture", id.0));
        let render = Arc::new(NullDevice::new(
            render_id.clone(),
            render_spec,
            Some(id),
            Some(Arc::clone(&loopback)),
            Arc::clone(&self.clock),
        ));
        let capture = Arc::new(NullDevice::new(
            capture_id.clone(),
            capture_spec,
            Some(id),
            Some(loopback),
            Arc::clone(&self.clock),
        ));
        let (ri, ci) = (render.info(), capture.info());
        inner.devices.insert(render_id.clone(), render);
        inner.devices.insert(capture_id.clone(), capture);
        inner.events.send(DeviceEvent::Added(ri));
        inner.events.send(DeviceEvent::Added(ci));
        let info = CableInfo {
            id,
            name,
            // `channels` est le raccourci documenté sur `format.channels` : il en sort,
            // il ne s'en écarte pas.
            channels: format.channels,
            format,
            active: true,
            render: render_id,
            capture: capture_id,
        };
        inner.cables.insert(id, info.clone());
        inner.events.send(DeviceEvent::CableChanged {
            id,
            info: Some(info.clone()),
        });
        Ok(info)
    }

    fn remove(&mut self, id: CableId) -> Result<(), CableError> {
        let info = {
            let mut inner = self.inner.lock().unwrap();
            inner.cables.remove(&id).ok_or(CableError::NotFound(id))?
        };
        self.remove_device(&info.render);
        self.remove_device(&info.capture);
        self.inner
            .lock()
            .unwrap()
            .events
            .send(DeviceEvent::CableChanged { id, info: None });
        Ok(())
    }

    /// Règle les canaux **sans toucher au reste du format** : un câble en 96 kHz PCM 24
    /// qui passe de 2 à 6 canaux reste en 96 kHz PCM 24.
    fn set_channels(
        &mut self,
        id: CableId,
        channels: ChannelCount,
    ) -> Result<CableInfo, CableError> {
        let old = self.get(id)?;
        self.set_format(
            id,
            CableFormat {
                channels,
                ..old.format
            },
        )
    }

    /// Applique le format et rend le câble : le backend simulé n'a **rien de plus à
    /// simuler**.
    ///
    /// Il n'a ni clé matérielle à écrire ni devnode à redémarrer, et surtout pas
    /// d'endpoint dont le format serait figé à la création — le refus « ce câble est
    /// connecté » de la vraie plateforme n'a donc ici aucun objet à protéger. La
    /// recréation reproduit tout de même le **court silence** : les périphériques
    /// disparaissent puis réapparaissent, et les tests du moteur voient la même séquence
    /// d'événements que sur une vraie machine.
    fn set_format(&mut self, id: CableId, format: CableFormat) -> Result<CableInfo, CableError> {
        let old = self.get(id)?;
        if old.format == format {
            return Ok(old);
        }
        self.remove(id)?;
        let mut inner = self.inner.lock().unwrap();
        inner.next_cable = id.0; // réutilise le numéro
        drop(inner);
        let info = self.create(CableSpec {
            name: Some(old.name),
            channels: format.channels,
            format: Some(format),
        })?;
        let mut inner = self.inner.lock().unwrap();
        inner.next_cable = inner.cables.keys().map(|c| c.0).max().unwrap_or(0) + 1;
        Ok(info)
    }

    fn rename(&mut self, id: CableId, name: &str) -> Result<CableInfo, CableError> {
        validate_cable_name(name)?;
        let mut inner = self.inner.lock().unwrap();
        let info = inner.cables.get_mut(&id).ok_or(CableError::NotFound(id))?;
        info.name = name.to_string();
        let info = info.clone();
        for dev_id in [&info.render, &info.capture] {
            if let Some(d) = inner.devices.get(dev_id) {
                d.rename(name);
            }
        }
        inner.events.send(DeviceEvent::CableChanged {
            id,
            info: Some(info.clone()),
        });
        Ok(info)
    }
}

#[cfg(test)]
mod tests;
