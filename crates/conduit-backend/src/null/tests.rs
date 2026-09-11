use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn fmt(channels: usize, block: usize) -> StreamFormat {
    StreamFormat {
        sample_rate: SampleRate::HZ_48000,
        channels,
        block_frames: block,
    }
}

fn counting_callback() -> (AudioCallback, Arc<AtomicU64>) {
    let n = Arc::new(AtomicU64::new(0));
    let n2 = Arc::clone(&n);
    (
        Box::new(
            move |io: &mut crate::device::StreamIo<'_>, _clock: &ClockInfo| {
                io.silence_output();
                n2.fetch_add(1, Ordering::Relaxed);
            },
        ),
        n,
    )
}

#[test]
fn enumeration_defaults_and_events() {
    let mut b = NullBackend::new();
    let rx = b.subscribe();
    assert_eq!(b.name(), "null");
    assert!(b.devices().unwrap().is_empty());
    let spk = b.add_device(NullDeviceSpec::render("Haut-parleurs"));
    let mic = b.add_device(NullDeviceSpec::capture("Micro"));
    let spk2 = b.add_device(NullDeviceSpec::render("Casque"));
    assert_eq!(spk.as_str(), "null:haut-parleurs");
    let devs = b.devices().unwrap();
    assert_eq!(devs.len(), 3);
    assert!(devs.iter().find(|d| d.id == spk).unwrap().is_default);
    assert!(!devs.iter().find(|d| d.id == spk2).unwrap().is_default);
    assert_eq!(b.default_device(DeviceDirection::Render), Some(spk.clone()));
    assert_eq!(
        b.default_device(DeviceDirection::Capture),
        Some(mic.clone())
    );
    let events: Vec<_> = rx.try_iter().collect();
    assert!(matches!(&events[0], DeviceEvent::Added(i) if i.id == spk));
    assert!(
        matches!(&events[1], DeviceEvent::DefaultChanged { direction: DeviceDirection::Render, id: Some(i) } if *i == spk)
    );
    assert_eq!(events.len(), 5);
    b.set_default(DeviceDirection::Render, Some(spk2.clone()))
        .unwrap();
    assert_eq!(
        rx.try_recv().unwrap(),
        DeviceEvent::DefaultChanged {
            direction: DeviceDirection::Render,
            id: Some(spk2.clone())
        }
    );
    assert!(matches!(
        b.set_default(DeviceDirection::Capture, Some(spk2.clone())),
        Err(BackendError::UnsupportedFormat { .. })
    ));
    assert!(matches!(
        b.set_default(DeviceDirection::Capture, Some("nope".into())),
        Err(BackendError::NotFound(_))
    ));
    // Même nom → identifiant dédoublonné.
    let dup = b.add_device(NullDeviceSpec::render("Casque"));
    assert_eq!(dup.as_str(), "null:casque#2");
    assert!(format!("{b:?}").contains("devices"));
}

#[test]
fn manual_clock_runs_callbacks_deterministically() {
    let mut b = NullBackend::new();
    let id = b.add_device(NullDeviceSpec::render("Sortie").layout(2, 480));
    let (cb, count) = counting_callback();
    let mut h = b.open(&id, fmt(2, 480), cb).unwrap();
    assert_eq!(h.format().block_frames, 480);
    assert_eq!(h.info().name, "Sortie");
    assert!(!h.is_running());
    assert_eq!(
        b.advance(Duration::from_millis(100)),
        0,
        "pas démarré : aucun rappel"
    );
    h.start().unwrap();
    assert!(h.is_running());
    // 10 ms par bloc : 1 s → 100 rappels (le premier à t = 100 ms, déjà dû).
    let n = b.advance(Duration::from_secs(1));
    assert_eq!(n, 101);
    assert_eq!(count.load(Ordering::Relaxed), 101);
    let clock = h.clock();
    assert_eq!(clock.position, 101 * 480);
    assert_eq!(clock.frames, 480);
    assert!(clock.timestamp_ns <= b.now_ns());
    h.stop().unwrap();
    assert_eq!(b.advance(Duration::from_secs(1)), 0);
    assert_eq!(b.device(&id).unwrap().callbacks(), 101);
}

#[test]
fn drift_changes_callback_rate_and_jitter_keeps_average() {
    let mut b = NullBackend::new();
    let fast = b.add_device(NullDeviceSpec::render("Rapide").drift(1000.0).layout(1, 48));
    let slow = b.add_device(
        NullDeviceSpec::render("Lent")
            .drift(-1000.0)
            .jitter(0.5)
            .layout(1, 48),
    );
    let (cb1, n1) = counting_callback();
    let (cb2, n2) = counting_callback();
    let mut h1 = b.open(&fast, fmt(1, 48), cb1).unwrap();
    let mut h2 = b.open(&slow, fmt(1, 48), cb2).unwrap();
    h1.start().unwrap();
    h2.start().unwrap();
    b.advance(Duration::from_secs(10));
    // 1 ms par bloc nominal : 10 000 blocs ± 10 (1000 ppm).
    let (a, c) = (
        n1.load(Ordering::Relaxed) as i64,
        n2.load(Ordering::Relaxed) as i64,
    );
    assert!((a - 10_011).abs() <= 2, "rapide : {a}");
    assert!((c - 9_991).abs() <= 3, "lent avec gigue : {c}");
}

#[test]
fn capture_signal_injection_and_render_recording() {
    let mut b = NullBackend::new();
    let mic = b.add_device(NullDeviceSpec::capture("Micro").layout(1, 480));
    let spk = b.add_device(NullDeviceSpec::render("Sortie").layout(2, 480));
    b.device(&mic).unwrap().set_signal(Signal::Sine {
        frequency: 1000.0,
        amplitude: 0.5,
    });
    let captured = Arc::new(Mutex::new(Vec::new()));
    let c2 = Arc::clone(&captured);
    let mut hm = b
        .open(
            &mic,
            fmt(1, 480),
            Box::new(move |io, _| c2.lock().unwrap().extend_from_slice(io.input.unwrap())),
        )
        .unwrap();
    let mut hs = b
        .open(
            &spk,
            fmt(2, 480),
            Box::new(|io, clock| {
                for (i, frame) in io.output.as_deref_mut().unwrap().chunks_mut(2).enumerate() {
                    frame[0] = (clock.position + i as u64) as f32;
                    frame[1] = -1.0;
                }
            }),
        )
        .unwrap();
    hm.start().unwrap();
    hs.start().unwrap();
    b.advance(Duration::from_millis(100));
    let cap = captured.lock().unwrap().clone();
    assert_eq!(cap.len(), 11 * 480);
    let zc = cap
        .windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count();
    assert!((zc as i64 - 220).abs() <= 2, "passages par zéro {zc}");
    assert!((cap.iter().fold(0.0f32, |m, x| m.max(x.abs())) - 0.5).abs() < 1e-3);
    let rec = b.device(&spk).unwrap().take_recorded();
    assert_eq!(rec.len(), 11 * 480 * 2);
    assert_eq!(rec[0], 0.0);
    assert_eq!(rec[2], 1.0);
    assert_eq!(rec[1], -1.0);
    assert_eq!(rec[rec.len() - 2], (11 * 480 - 1) as f32);
    assert!(b.device(&spk).unwrap().take_recorded().is_empty());
    b.device(&spk).unwrap().set_max_record_frames(10);
    b.advance(Duration::from_millis(100));
    assert_eq!(b.device(&spk).unwrap().take_recorded().len(), 20);
    b.device(&mic).unwrap().set_signal(Signal::Dc(0.25));
    captured.lock().unwrap().clear();
    b.advance(Duration::from_millis(10));
    assert!(captured.lock().unwrap().iter().all(|&x| x == 0.25));
    let mut custom_calls = 0u64;
    b.device(&mic)
        .unwrap()
        .set_signal(Signal::Custom(Box::new(move |buf, _pos, ch| {
            custom_calls += 1;
            assert_eq!(ch, 1);
            buf.fill(custom_calls as f32);
        })));
    captured.lock().unwrap().clear();
    b.advance(Duration::from_millis(20));
    assert_eq!(captured.lock().unwrap()[0], 1.0);
    assert!(format!("{:?}", Signal::Silence).contains("Silence"));
    assert!(format!("{:?}", b.device(&mic).unwrap()).contains("callbacks"));
}

#[test]
fn open_validates_format_and_busy() {
    let mut b = NullBackend::new();
    let id = b.add_device(NullDeviceSpec::render("S").layout(2, 256));
    let (cb, _) = counting_callback();
    let err = b.open(&id, fmt(1, 256), cb).unwrap_err();
    assert!(
        matches!(err, BackendError::UnsupportedFormat { .. }),
        "{err}"
    );
    let (cb, _) = counting_callback();
    let bad_rate = StreamFormat {
        sample_rate: SampleRate::HZ_96000,
        channels: 2,
        block_frames: 256,
    };
    assert!(matches!(
        b.open(&id, bad_rate, cb),
        Err(BackendError::UnsupportedFormat { .. })
    ));
    let (cb, _) = counting_callback();
    let h = b.open(&id, fmt(2, 0), cb).unwrap();
    assert_eq!(h.format().block_frames, 1, "bloc minimal 1");
    let (cb, _) = counting_callback();
    assert!(matches!(
        b.open(&id, fmt(2, 256), cb),
        Err(BackendError::Busy(_))
    ));
    drop(h);
    let (cb, _) = counting_callback();
    let reopened = b
        .open(&id, fmt(2, 256), cb)
        .expect("réouverture après fermeture");
    let (cb, _) = counting_callback();
    assert!(matches!(
        b.open(&"absent".into(), fmt(2, 256), cb),
        Err(BackendError::NotFound(_))
    ));
    assert!(format!("{:?}", b.open(&id, fmt(2, 256), counting_callback().0)).contains("Busy"));
    assert!(format!("{reopened:?}").contains("NullHandle"));
}

#[test]
fn hot_unplug_stops_stream_and_replug_works() {
    let mut b = NullBackend::new();
    let rx = b.subscribe();
    let id = b.add_device(NullDeviceSpec::render("USB").layout(2, 480));
    let other = b.add_device(NullDeviceSpec::render("Interne").layout(2, 480));
    let (cb, count) = counting_callback();
    let mut h = b.open(&id, fmt(2, 480), cb).unwrap();
    h.start().unwrap();
    b.advance(Duration::from_millis(50));
    let before = count.load(Ordering::Relaxed);
    assert!(before > 0);
    rx.try_iter().count();
    assert!(b.remove_device(&id));
    assert!(!b.remove_device(&id));
    assert!(!h.is_running());
    assert_eq!(
        h.start().unwrap_err(),
        BackendError::Disconnected(id.clone())
    );
    b.advance(Duration::from_millis(50));
    assert_eq!(
        count.load(Ordering::Relaxed),
        before,
        "plus aucun rappel après retrait"
    );
    let events: Vec<_> = rx.try_iter().collect();
    assert_eq!(events[0], DeviceEvent::Removed { id: id.clone() });
    assert_eq!(
        events[1],
        DeviceEvent::DefaultChanged {
            direction: DeviceDirection::Render,
            id: Some(other.clone())
        }
    );
    assert_eq!(b.devices().unwrap().len(), 1);
    assert!(b.device(&id).is_none());
    // Réapparition : même nom → même identifiant (le précédent est libéré).
    let again = b.add_device(NullDeviceSpec::render("USB").layout(2, 480));
    assert_eq!(again, id);
    drop(h);
    let (cb, count2) = counting_callback();
    let mut h2 = b.open(&again, fmt(2, 480), cb).unwrap();
    h2.start().unwrap();
    b.advance(Duration::from_millis(50));
    assert!(count2.load(Ordering::Relaxed) > 0);
}

#[test]
fn cables_loop_render_to_capture() {
    let mut b = NullBackend::new();
    let rx = b.subscribe();
    let cc = b.cable_control().unwrap();
    assert_eq!(cc.max_cables(), NullBackend::DEFAULT_MAX_CABLES);
    let c1 = cc.create(CableSpec::default()).unwrap();
    assert_eq!(c1.id, CableId(1));
    assert_eq!(c1.name, "Conduit 1");
    assert!(c1.active);
    let c2 = cc
        .create(CableSpec {
            name: Some("Musique".into()),
            channels: ChannelCount::MONO,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(c2.id, CableId(2));
    assert_eq!(cc.list().unwrap().len(), 2);
    assert_eq!(cc.get(CableId(2)).unwrap().channels, ChannelCount::MONO);
    // `channels` est un raccourci sur `format.channels`, jamais une seconde vérité.
    for cable in cc.list().unwrap() {
        assert_eq!(cable.channels, cable.format.channels, "{}", cable.name);
    }
    assert!(matches!(
        cc.get(CableId(7)),
        Err(CableError::NotFound(CableId(7)))
    ));
    assert!(matches!(
        cc.create(CableSpec {
            name: Some("a/b".into()),
            ..Default::default()
        }),
        Err(CableError::InvalidName(_))
    ));
    let devs = b.devices().unwrap();
    assert_eq!(devs.len(), 4);
    assert!(devs.iter().all(|d| d.cable.is_some()));
    let events: Vec<_> = rx.try_iter().collect();
    assert!(events.iter().any(|e| matches!(
        e,
        DeviceEvent::CableChanged {
            id: CableId(1),
            info: Some(_)
        }
    )));
    // Boucle locale : ce qui est rendu sur c1.render ressort sur c1.capture.
    let (render_id, capture_id) = (c1.render.clone(), c1.capture.clone());
    let mut hr = b
        .open(
            &render_id,
            StreamFormat {
                sample_rate: SampleRate::HZ_48000,
                channels: 2,
                block_frames: 480,
            },
            Box::new(|io, clock| {
                for (i, f) in io.output.as_deref_mut().unwrap().chunks_mut(2).enumerate() {
                    f[0] = (clock.position + i as u64) as f32;
                    f[1] = 0.5;
                }
            }),
        )
        .unwrap();
    let got = Arc::new(Mutex::new(Vec::new()));
    let g2 = Arc::clone(&got);
    let mut hc = b
        .open(
            &capture_id,
            StreamFormat {
                sample_rate: SampleRate::HZ_48000,
                channels: 2,
                block_frames: 480,
            },
            Box::new(move |io, _| g2.lock().unwrap().extend_from_slice(io.input.unwrap())),
        )
        .unwrap();
    hc.start().unwrap();
    hr.start().unwrap();
    b.advance(Duration::from_millis(100));
    let got = got.lock().unwrap();
    // Les deux flux ont la même horloge ; selon l'ordre d'exécution, le premier bloc
    // capturé peut être du silence, puis viennent les données rendues (canal 1 = 0,5).
    let first_frame = got.chunks(2).position(|f| f[1] == 0.5).unwrap();
    assert!(first_frame <= 480, "décalage {first_frame}");
    let tail = &got[first_frame * 2..];
    assert!(tail.chunks(2).take(2000).all(|f| f[1] == 0.5));
    let seq: Vec<f32> = tail.chunks(2).take(100).map(|f| f[0]).collect();
    assert_eq!(seq[0], 0.0, "première trame rendue = position 0");
    // Continuité : chaque échantillon suit le précédent.
    assert!(seq.windows(2).all(|w| w[1] == w[0] + 1.0), "{seq:?}");
}

#[test]
fn cable_remove_rename_channels_and_limit() {
    let mut b = NullBackend::new().with_max_cables(2);
    let rx = b.subscribe();
    let cc = b.cable_control().unwrap();
    let c1 = cc.create(CableSpec::default()).unwrap();
    let _c2 = cc.create(CableSpec::default()).unwrap();
    assert_eq!(
        cc.create(CableSpec::default()).unwrap_err(),
        CableError::LimitReached { max: 2 }
    );
    let renamed = cc.rename(c1.id, "Jeu").unwrap();
    assert_eq!(renamed.name, "Jeu");
    assert!(matches!(
        cc.rename(c1.id, ""),
        Err(CableError::InvalidName(_))
    ));
    assert!(matches!(
        cc.rename(CableId(9), "x"),
        Err(CableError::NotFound(_))
    ));
    let same = cc.set_channels(c1.id, ChannelCount::STEREO).unwrap();
    assert_eq!(same, renamed);
    let changed = cc
        .set_channels(c1.id, ChannelCount::new(6).unwrap())
        .unwrap();
    assert_eq!(changed.id, c1.id, "le numéro est conservé");
    assert_eq!(changed.name, "Jeu");
    assert_eq!(changed.channels.get(), 6);
    // Régler les canaux ne touche pas au reste du format.
    assert_eq!(changed.format.sample_rate, c1.format.sample_rate);
    assert_eq!(changed.format.depth, c1.format.depth);
    assert_eq!(changed.format.channels, changed.channels);
    assert_eq!(cc.list().unwrap().len(), 2);
    cc.remove(c1.id).unwrap();
    assert!(matches!(cc.remove(c1.id), Err(CableError::NotFound(_))));
    assert_eq!(cc.list().unwrap().len(), 1);
    // Un troisième câble est de nouveau possible et reçoit un nouveau numéro.
    let c3 = cc.create(CableSpec::default()).unwrap();
    assert_eq!(c3.id, CableId(3));
    let devs = b.devices().unwrap();
    assert_eq!(devs.len(), 4);
    assert!(devs
        .iter()
        .find(|d| d.id == c3.render)
        .unwrap()
        .name
        .contains("Conduit 3"));
    let events: Vec<_> = rx.try_iter().collect();
    assert!(events.iter().any(|e| matches!(
        e,
        DeviceEvent::CableChanged {
            id: CableId(1),
            info: None
        }
    )));
    assert!(events.iter().any(|e| matches!(e, DeviceEvent::CableChanged { id: CableId(1), info: Some(i) } if i.name == "Jeu")));
}

/// `set_format` applique les trois champs, conserve le numéro et le nom, et se voit
/// jusque dans les périphériques publiés.
///
/// C'est ce dernier point qui compte : un format que seul le `CableInfo` porterait ne
/// prouverait rien. Ici la fréquence du câble est celle de ses deux périphériques, et
/// c'est la propriété que le vrai pilote obtient en redémarrant son devnode.
#[test]
fn cable_set_format_applies_and_reports() {
    let mut b = NullBackend::new();
    let cc = b.cable_control().unwrap();
    let c1 = cc.create(CableSpec::default()).unwrap();
    assert_eq!(c1.format, CableFormat::default());

    // Le même format : rien n'est recréé, l'état est rendu tel quel.
    assert_eq!(cc.set_format(c1.id, CableFormat::default()).unwrap(), c1);

    let voulu = CableFormat {
        sample_rate: SampleRate::HZ_96000,
        depth: crate::cable::SampleDepth::Pcm24,
        channels: ChannelCount::new(6).unwrap(),
    };
    let apres = cc.set_format(c1.id, voulu).unwrap();
    assert_eq!(apres.id, c1.id, "le numéro est conservé");
    assert_eq!(apres.name, c1.name);
    assert_eq!(apres.format, voulu);
    assert_eq!(
        apres.channels, voulu.channels,
        "le raccourci suit le format"
    );
    assert_eq!(cc.get(c1.id).unwrap().format, voulu);

    // Un câble inconnu est nommé, jamais confondu avec un succès.
    assert!(matches!(
        cc.set_format(CableId(9), voulu),
        Err(CableError::NotFound(CableId(9)))
    ));

    // Le format se voit dans les périphériques publiés, pas seulement dans le `CableInfo`.
    for device in b.devices().unwrap() {
        if device.cable == Some(c1.id) {
            assert_eq!(device.sample_rate, SampleRate::HZ_96000, "{}", device.id);
            assert_eq!(device.channels, 6, "{}", device.id);
        }
    }
}

/// Un `CableSpec` qui porte un format l'applique **à la création**, et `channels` est
/// alors ignoré — c'est ce que la documentation du champ promet.
#[test]
fn cable_spec_format_wins_over_channels() {
    let mut b = NullBackend::new();
    let cc = b.cable_control().unwrap();
    let voulu = CableFormat {
        sample_rate: SampleRate::HZ_44100,
        depth: crate::cable::SampleDepth::Pcm16,
        channels: ChannelCount::new(4).unwrap(),
    };
    let info = cc
        .create(CableSpec {
            name: None,
            // Contredit volontairement le format : c'est le format qui gagne.
            channels: ChannelCount::MONO,
            format: Some(voulu),
        })
        .unwrap();
    assert_eq!(info.format, voulu);
    assert_eq!(info.channels, voulu.channels);
}

#[test]
fn clone_shares_devices_and_clock() {
    let mut a = NullBackend::new();
    let b = a.clone();
    let id = b.add_device(NullDeviceSpec::render("Partagé").layout(1, 48));
    assert_eq!(a.devices().unwrap().len(), 1);
    let (cb, count) = counting_callback();
    let mut h = a.open(&id, fmt(1, 48), cb).unwrap();
    h.start().unwrap();
    b.advance(Duration::from_millis(10));
    assert_eq!(count.load(Ordering::Relaxed), 11);
    assert_eq!(a.now_ns(), b.now_ns());
    assert!(!b.timer_running());
}

#[test]
fn timer_mode_runs_callbacks_in_real_time() {
    let mut b = NullBackend::new();
    let id = b.add_device(NullDeviceSpec::render("T").layout(1, 48));
    let (cb, count) = counting_callback();
    let mut h = b.open(&id, fmt(1, 48), cb).unwrap();
    h.start().unwrap();
    assert!(!b.timer_running());
    b.start_timer();
    b.start_timer();
    assert!(b.timer_running());
    let t0 = Instant::now();
    while count.load(Ordering::Relaxed) < 50 && t0.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let elapsed = t0.elapsed();
    let n = count.load(Ordering::Relaxed);
    assert!(n >= 50, "{n} rappels en {elapsed:?}");
    assert!(
        elapsed >= Duration::from_millis(40),
        "50 blocs de 1 ms ne peuvent pas prendre {elapsed:?}"
    );
    b.stop_timer();
    assert!(!b.timer_running());
    let after = count.load(Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(
        count.load(Ordering::Relaxed),
        after,
        "plus de rappel après l'arrêt du timer"
    );
    b.start_timer();
    drop(b);
}
