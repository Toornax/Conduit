//! Le fil dédié qui fait tourner la boucle PipeWire.
//!
//! Rien de ce que fournit `pipewire-rs` n'est `Send` : contexte, registre, flux et
//! écouteurs vivent tous ici. Le reste du programme parle à ce fil par
//! `pw::channel` (commandes) et lit l'état par `Arc<Mutex<…>>`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use conduit_backend::event::EventBroadcaster;
use conduit_backend::{
    AudioCallback, BackendError, DeviceDirection, DeviceEvent, DeviceId, StreamFormat,
};
use pipewire as pw;
use pw::metadata::{Metadata, MetadataListener};
use pw::types::ObjectType;

use crate::devices::{device_from_props, metadata_name, Shared};
use crate::stream::{OpenStream, StreamShared};

/// Identifiant local d'un flux ouvert.
pub(crate) type StreamKey = u64;

/// Ordre envoyé au fil de boucle.
pub(crate) enum Command {
    /// Ouvrir un flux (boîte : la variante serait sinon bien plus grosse que les autres).
    Open(Box<OpenRequest>),
    /// Démarrer ou arrêter les rappels d'un flux.
    SetActive {
        key: StreamKey,
        active: bool,
        reply: mpsc::Sender<Result<(), BackendError>>,
    },
    /// Fermer un flux et confirmer qu'aucun rappel ne peut plus survenir.
    Close {
        key: StreamKey,
        reply: mpsc::Sender<()>,
    },
    /// Arrêter la boucle.
    Quit,
}

/// Demande d'ouverture, préparée hors du fil de boucle.
pub(crate) struct OpenRequest {
    pub(crate) key: StreamKey,
    pub(crate) id: DeviceId,
    pub(crate) format: StreamFormat,
    pub(crate) callback: AudioCallback,
    pub(crate) shared: Arc<StreamShared>,
    pub(crate) reply: mpsc::Sender<Result<(), BackendError>>,
}

/// Nom du démon visé, pour les messages d'erreur.
pub(crate) fn remote_label(remote: Option<&str>) -> String {
    remote
        .map(str::to_string)
        .or_else(|| {
            std::env::var("PIPEWIRE_REMOTE")
                .ok()
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "pipewire-0".to_string())
}

/// Corps du fil de boucle. Ne retourne qu'à l'arrêt de la boucle.
///
/// `ready` reçoit `Ok(())` quand l'énumération initiale est terminée, ou l'erreur
/// qui a empêché la connexion.
pub(crate) fn run(
    remote: Option<String>,
    shared: Arc<Mutex<Shared>>,
    events: Arc<Mutex<EventBroadcaster>>,
    receiver: pw::channel::Receiver<Command>,
    ready: mpsc::Sender<Result<(), String>>,
) {
    let main_loop = match pw::main_loop::MainLoopRc::new(None) {
        Ok(l) => l,
        Err(e) => {
            let _ = ready.send(Err(format!("boucle PipeWire non créée : {e}")));
            return;
        }
    };
    let context = match pw::context::ContextRc::new(&main_loop, None) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(format!("contexte PipeWire non créé : {e}")));
            return;
        }
    };
    let props = remote.as_deref().map(|name| {
        let mut props = pw::properties::PropertiesBox::new();
        props.insert(*pw::keys::REMOTE_NAME, name);
        props
    });
    let core = match context.connect_rc(props) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            return;
        }
    };
    let registry = match core.get_registry_rc() {
        Ok(r) => r,
        Err(e) => {
            let _ = ready.send(Err(format!("registre PipeWire inaccessible : {e}")));
            return;
        }
    };

    // La synchronisation est demandée avant d'écouter : la réponse n'arrivera
    // qu'une fois la boucle démarrée, après le premier lot de globaux.
    let pending = match core.sync(0) {
        Ok(seq) => seq,
        Err(e) => {
            let _ = ready.send(Err(format!("synchronisation PipeWire impossible : {e}")));
            return;
        }
    };

    let ready = Rc::new(RefCell::new(Some(ready)));
    let streams: Rc<RefCell<HashMap<StreamKey, OpenStream>>> =
        Rc::new(RefCell::new(HashMap::new()));
    let metadata: Rc<RefCell<Option<(u32, Metadata, MetadataListener)>>> =
        Rc::new(RefCell::new(None));

    let _core_listener = {
        let ready = Rc::clone(&ready);
        let quit = main_loop.clone();
        core.add_listener_local()
            .done(move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq == pending {
                    if let Some(tx) = ready.borrow_mut().take() {
                        let _ = tx.send(Ok(()));
                    }
                }
            })
            .error(move |id, _seq, res, message| {
                if id == pw::core::PW_ID_CORE {
                    // Erreur fatale du cœur : la boucle n'a plus rien à faire.
                    let _ = res;
                    let _ = message;
                    quit.quit();
                }
            })
            .register()
    };

    let _registry_listener = {
        let shared_add = Arc::clone(&shared);
        let events_add = Arc::clone(&events);
        let registry_weak = registry.downgrade();
        let metadata_add = Rc::clone(&metadata);
        let shared_meta = Arc::clone(&shared);
        let events_meta = Arc::clone(&events);

        let shared_rm = Arc::clone(&shared);
        let events_rm = Arc::clone(&events);
        let streams_rm = Rc::clone(&streams);
        let metadata_rm = Rc::clone(&metadata);

        registry
            .add_listener_local()
            .global(move |global| {
                let Some(props) = global.props else {
                    return;
                };
                match global.type_ {
                    ObjectType::Node => {
                        let Some(info) = device_from_props(props) else {
                            return;
                        };
                        let added = shared_add.lock().expect("registre").insert(global.id, info);
                        if let Some(info) = added {
                            events_add
                                .lock()
                                .expect("abonnés")
                                .send(DeviceEvent::Added(info));
                        }
                    }
                    ObjectType::Metadata if props.get("metadata.name") == Some("default") => {
                        let Some(registry) = registry_weak.upgrade() else {
                            return;
                        };
                        let Ok(proxy) = registry.bind::<Metadata, _>(global) else {
                            return;
                        };
                        let shared = Arc::clone(&shared_meta);
                        let events = Arc::clone(&events_meta);
                        let listener = proxy
                            .add_listener_local()
                            .property(move |_subject, key, _type, value| {
                                on_default_metadata(&shared, &events, key, value);
                                0
                            })
                            .register();
                        *metadata_add.borrow_mut() = Some((global.id, proxy, listener));
                    }
                    _ => {}
                }
            })
            .global_remove(move |id| {
                let is_metadata = metadata_rm.borrow().as_ref().map(|(gid, _, _)| *gid) == Some(id);
                if is_metadata {
                    metadata_rm.borrow_mut().take();
                    return;
                }
                // Le verrou du registre est relâché avant de diffuser les événements.
                let removed = {
                    let mut guard = shared_rm.lock().expect("registre");
                    guard.remove_global(id).map(|info| {
                        let cleared = info.is_default && guard.set_default(info.direction, None);
                        (info, cleared)
                    })
                };
                let Some((info, default_cleared)) = removed else {
                    return;
                };
                for stream in streams_rm.borrow().values() {
                    if stream.device == info.id {
                        stream.shared.mark_disconnected();
                    }
                }
                let mut events = events_rm.lock().expect("abonnés");
                events.send(DeviceEvent::Removed {
                    id: info.id.clone(),
                });
                if default_cleared {
                    events.send(DeviceEvent::DefaultChanged {
                        direction: info.direction,
                        id: None,
                    });
                }
            })
            .register()
    };

    let _commands = {
        let core = core.clone();
        let shared = Arc::clone(&shared);
        let streams = Rc::clone(&streams);
        let quit = main_loop.clone();
        receiver.attach(main_loop.loop_(), move |command| match command {
            Command::Open(request) => {
                let (reply, result) = handle_open(&core, &shared, &streams, *request);
                let _ = reply.send(result);
            }
            Command::SetActive { key, active, reply } => {
                let result = match streams.borrow().get(&key) {
                    Some(stream) => stream.set_active(active),
                    None => Err(BackendError::Platform("flux déjà fermé".into())),
                };
                let _ = reply.send(result);
            }
            Command::Close { key, reply } => {
                if let Some(stream) = streams.borrow_mut().remove(&key) {
                    stream.close();
                    shared.lock().expect("registre").mark_closed(&stream.device);
                }
                let _ = reply.send(());
            }
            Command::Quit => {
                for (_, stream) in streams.borrow_mut().drain() {
                    stream.close();
                }
                quit.quit();
            }
        })
    };

    main_loop.run();
}

/// Applique une demande d'ouverture. Retourne l'émetteur de réponse et le résultat.
type OpenOutcome = (
    mpsc::Sender<Result<(), BackendError>>,
    Result<(), BackendError>,
);

fn handle_open(
    core: &pw::core::CoreRc,
    shared: &Arc<Mutex<Shared>>,
    streams: &Rc<RefCell<HashMap<StreamKey, OpenStream>>>,
    request: OpenRequest,
) -> OpenOutcome {
    let OpenRequest {
        key,
        id,
        format,
        callback,
        shared: stream_shared,
        reply,
    } = request;
    let info = {
        let mut guard = shared.lock().expect("registre");
        match guard.get(&id).cloned() {
            None => return (reply, Err(BackendError::NotFound(id))),
            Some(_) if guard.is_open(&id) => return (reply, Err(BackendError::Busy(id))),
            Some(info) => {
                guard.mark_open(&id);
                info
            }
        }
    };
    match crate::stream::open(core, &info, format, callback, stream_shared) {
        Ok(stream) => {
            streams.borrow_mut().insert(key, stream);
            (reply, Ok(()))
        }
        Err(e) => {
            shared.lock().expect("registre").mark_closed(&id);
            (reply, Err(e))
        }
    }
}

/// Réagit à `default.audio.sink` / `default.audio.source`.
fn on_default_metadata(
    shared: &Arc<Mutex<Shared>>,
    events: &Arc<Mutex<EventBroadcaster>>,
    key: Option<&str>,
    value: Option<&str>,
) {
    let direction = match key {
        Some("default.audio.sink") => DeviceDirection::Render,
        Some("default.audio.source") => DeviceDirection::Capture,
        _ => return,
    };
    let id = value.and_then(metadata_name).map(DeviceId::new);
    let changed = shared
        .lock()
        .expect("registre")
        .set_default(direction, id.clone());
    if changed {
        events
            .lock()
            .expect("abonnés")
            .send(DeviceEvent::DefaultChanged { direction, id });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_label_falls_back_on_the_environment() {
        assert_eq!(remote_label(Some("pipewire-1")), "pipewire-1");
        // Sans argument ni variable, c'est le nom par défaut de PipeWire.
        if std::env::var_os("PIPEWIRE_REMOTE").is_none() {
            assert_eq!(remote_label(None), "pipewire-0");
        }
    }

    #[test]
    fn default_metadata_updates_shared_state() {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let events = Arc::new(Mutex::new(EventBroadcaster::default()));
        let rx = events.lock().unwrap().subscribe();
        on_default_metadata(
            &shared,
            &events,
            Some("default.audio.sink"),
            Some("{\"name\":\"Null-Sink\"}"),
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            DeviceEvent::DefaultChanged {
                direction: DeviceDirection::Render,
                id: Some("Null-Sink".into()),
            }
        );
        // Une clé sans intérêt ne produit rien.
        on_default_metadata(&shared, &events, Some("clock.rate"), Some("48000"));
        assert!(rx.try_recv().is_err());
        // Effacement.
        on_default_metadata(&shared, &events, Some("default.audio.sink"), None);
        assert_eq!(
            rx.try_recv().unwrap(),
            DeviceEvent::DefaultChanged {
                direction: DeviceDirection::Render,
                id: None,
            }
        );
    }
}
