#![no_main]

use datalog::{Strings, parse};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(src) = core::str::from_utf8(data) {
        // The contract: a Diagnostic or a Program, never an abort.
        let _ = parse(src, &mut Strings::new());
    }
});
