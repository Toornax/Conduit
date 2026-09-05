//! Cible cargo-fuzz : le décodeur de trames ne panique jamais (M0-74).
//! `cargo +nightly fuzz run decoder` depuis `crates/conduit-protocol`.
#![no_main]

use conduit_protocol::framing::Decoder;
use conduit_protocol::wire::Message;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut d = Decoder::with_max_len(1 << 16);
    for chunk in data.chunks(97) {
        d.feed(chunk);
        loop {
            match d.next_message::<Message>() {
                Ok(Some(_)) => continue,
                Ok(None) => break,
                Err(_) => return,
            }
        }
    }
});
