//! Parsing untrusted input produces a `Diagnostic`, never an abort.
//!
//! The spec asks for `cargo fuzz run parser`. This is the part that runs on
//! every commit with no nightly toolchain and no extra tool: a seeded generator
//! over bytes, token soup and mutations of valid programs. `fuzz/` holds the
//! libfuzzer target for the longer campaign.

use datalog::diag::Status;
use datalog::{Strings, parse};

/// xorshift64*, so the corpus is identical on every machine and every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            usize::try_from(self.next() % n as u64).unwrap_or(0)
        }
    }
}

/// Parse must return, and must return a typed rejection rather than panicking.
fn survives(src: &str) {
    match parse(src, &mut Strings::new()) {
        Ok(_) => {}
        Err(d) => assert!(
            matches!(d.status, Status::InvalidQuery),
            "parsing rejects with invalid-query, got {} for {src:?}",
            d.status
        ),
    }
}

#[test]
fn random_bytes_never_panic() {
    let mut rng = Rng(0x5EED_1234_ABCD_0001);
    for _ in 0..20_000 {
        let len = rng.below(48);
        let bytes: Vec<u8> = (0..len)
            .map(|_| u8::try_from(rng.next() % 128).unwrap_or(0))
            .collect();
        let src = String::from_utf8_lossy(&bytes);
        survives(&src);
    }
}

#[test]
fn random_token_soup_never_panics() {
    const PIECES: [&str; 24] = [
        "r", "X", "_", "(", ")", ",", ".", ":-", "?-", "!", "=", "!=", "<", "<=", ">", ">=", "+",
        "-", "*", "/", "count", "{", "}", "\"s\"",
    ];
    let mut rng = Rng(0x5EED_1234_ABCD_0002);
    for _ in 0..20_000 {
        let len = rng.below(24);
        let src: String = (0..len)
            .map(|_| PIECES.get(rng.below(PIECES.len())).copied().unwrap_or("."))
            .collect::<Vec<_>>()
            .join(" ");
        survives(&src);
    }
}

#[test]
fn mutations_of_valid_programs_never_panic() {
    const SEEDS: [&str; 6] = [
        r#"def("s", "f", "function", "get")."#,
        "reaches(A, B) :- edge(A, B).\nreaches(A, C) :- reaches(A, B), edge(B, C).",
        "hot(S, N) :- def(S), N = count{ C : calls(C, S) }, N > 10.",
        "sym(F, L, S) :- span(S, F, A, B), between(A, B, L).",
        r#"is_test(F) :- file(F), match(F, "_test\\.go$")."#,
        "?- def(S, F, _, N), at(S, F, L).",
    ];
    let mut rng = Rng(0x5EED_1234_ABCD_0003);
    for _ in 0..20_000 {
        let seed = SEEDS.get(rng.below(SEEDS.len())).copied().unwrap_or(".");
        let mut bytes = seed.as_bytes().to_vec();
        for _ in 0..=rng.below(4) {
            if bytes.is_empty() {
                break;
            }
            let at = rng.below(bytes.len());
            match rng.below(3) {
                0 => {
                    bytes.remove(at);
                }
                1 => {
                    if let Some(slot) = bytes.get_mut(at) {
                        *slot = u8::try_from(rng.next() % 128).unwrap_or(b' ');
                    }
                }
                _ => bytes.insert(at, u8::try_from(rng.next() % 128).unwrap_or(b' ')),
            }
        }
        survives(&String::from_utf8_lossy(&bytes));
    }
}

#[test]
fn deeply_nested_input_does_not_overflow_the_stack() {
    let deep = format!(
        "r(X) :- {}.",
        "count{ Y : e(Y) }, ".repeat(200).trim_end_matches(", ")
    );
    survives(&deep);
    survives(&"(".repeat(10_000));
    survives(&format!("r({}).", "X, ".repeat(5_000)));
}
