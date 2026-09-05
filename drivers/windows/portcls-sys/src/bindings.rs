//! Bindings générés par `bindgen` (`build.rs`) depuis `wrapper.h` : mode C, vtables plates.
//!
//! Le code généré ne respecte ni les conventions de nommage Rust ni les lints du
//! workspace ; les `allow` ci-dessous sont limités à ce module. Les assertions de
//! disposition de bindgen (`const _: () = { ["…"][offset_of!(…) - N]; }`) indexent un
//! tableau et soustraient dans un bloc `const` : elles déclenchent `indexing_slicing`
//! et `arithmetic_side_effects` (lints `restriction`, hors `clippy::all`), alors
//! qu'elles sont justement le mécanisme d'échec à la compilation voulu.

#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    missing_docs,
    missing_debug_implementations,
    unsafe_op_in_unsafe_fn,
    unnecessary_transmutes,
    clippy::all,
    clippy::undocumented_unsafe_blocks,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

// Vtable corrigée à la main (voir `fixups`) : bindgen bloque la sienne et référence ce nom.
use crate::fixups::IPortClsVersionVtbl;

include!(concat!(env!("OUT_DIR"), "/portcls.rs"));
include!(concat!(env!("OUT_DIR"), "/guids.rs"));
