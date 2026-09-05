//! Cible cargo-fuzz : l'analyseur de configuration TOML ne panique jamais.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        let _ = conduitd::config::Config::parse(text);
    }
});
