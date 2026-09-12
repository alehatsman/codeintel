//! The dictionary: stable ids, an append-only file, and a corrupt index that
//! says so instead of handing back a different string than it stored.

use std::fs;

use datalog::atom::{INT_MAX, STR_MIN, is_int};
use facts::{Dict, Interner};
use tempfile::TempDir;

fn interner() -> (TempDir, Interner) {
    let dir = TempDir::new().expect("a temp dir");
    let interner = Interner::open(dir.path()).expect("opens");
    (dir, interner)
}

#[test]
fn the_empty_string_is_the_first_string_atom() {
    let (_dir, interner) = interner();
    assert_eq!(interner.lookup(""), Some(STR_MIN));
    assert_eq!(interner.resolve(STR_MIN), Some(""));
}

#[test]
fn interning_dedupes_and_ids_start_above_the_integer_range() {
    let (_dir, mut interner) = interner();
    let a = interner.intern("src/store.rs").expect("room");
    let b = interner.intern("src/store.rs").expect("room");
    assert_eq!(a, b);
    assert!(!is_int(a), "a string atom is never an integer atom");
    assert!(a > INT_MAX);
}

#[test]
fn ids_survive_a_flush_and_a_reopen() {
    let dir = TempDir::new().expect("a temp dir");
    let mut first = Interner::open(dir.path()).expect("opens");
    let get = first.intern("get").expect("room");
    let set = first.intern("set").expect("room");
    first.flush().expect("flushes");

    let mut second = Interner::open(dir.path()).expect("reopens");
    assert_eq!(second.lookup("get"), Some(get));
    assert_eq!(second.resolve(set), Some("set"));
    // Appending after a reopen continues the id space rather than reusing it.
    let new = second.intern("insert").expect("room");
    assert!(new > set);
    assert_eq!(second.len(), 4, "\"\", get, set, insert");
}

#[test]
fn a_second_flush_appends_rather_than_rewrites() {
    let dir = TempDir::new().expect("a temp dir");
    let mut interner = Interner::open(dir.path()).expect("opens");
    let first = interner.intern("alpha").expect("room");
    interner.flush().expect("flushes");
    let bin_after_one = fs::metadata(dir.path().join("dict.bin"))
        .expect("bin")
        .len();

    let second = interner.intern("beta").expect("room");
    interner.flush().expect("flushes again");
    let bin_after_two = fs::metadata(dir.path().join("dict.bin"))
        .expect("bin")
        .len();

    assert!(bin_after_two > bin_after_one, "the second flush appended");
    let reopened = Interner::open(dir.path()).expect("reopens");
    assert_eq!(reopened.resolve(first), Some("alpha"));
    assert_eq!(reopened.resolve(second), Some("beta"));
}

#[test]
fn flushing_twice_with_nothing_new_changes_nothing() {
    let dir = TempDir::new().expect("a temp dir");
    let mut interner = Interner::open(dir.path()).expect("opens");
    interner.intern("alpha").expect("room");
    interner.flush().expect("flushes");
    let before = fs::read(dir.path().join("dict.idx")).expect("idx");
    interner.flush().expect("flushes again");
    let after = fs::read(dir.path().join("dict.idx")).expect("idx");
    assert_eq!(before, after);
}

#[test]
fn interning_without_flushing_leaves_the_disk_alone() {
    let dir = TempDir::new().expect("a temp dir");
    let mut interner = Interner::open(dir.path()).expect("opens");
    interner.intern("a-name-no-corpus-has").expect("room");
    assert_eq!(interner.pending(), 2, "the empty string and the new name");
    assert!(
        !dir.path().join("dict.bin").exists(),
        "a query must not write to the index"
    );
}

#[test]
fn the_offset_table_is_u64_wide() {
    let dir = TempDir::new().expect("a temp dir");
    let mut interner = Interner::open(dir.path()).expect("opens");
    interner.intern("x").expect("room");
    interner.flush().expect("flushes");
    let idx = fs::read(dir.path().join("dict.idx")).expect("idx");
    // Two strings ("" and "x") means three offsets, eight bytes each.
    assert_eq!(idx.len(), 3 * 8);
}

#[test]
fn non_ascii_round_trips_by_bytes() {
    let dir = TempDir::new().expect("a temp dir");
    let mut interner = Interner::open(dir.path()).expect("opens");
    let atom = interner.intern("héllo→世界").expect("room");
    interner.flush().expect("flushes");
    let reopened = Interner::open(dir.path()).expect("reopens");
    assert_eq!(reopened.resolve(atom), Some("héllo→世界"));
}

#[test]
fn an_unknown_atom_resolves_to_nothing() {
    let (_dir, interner) = interner();
    assert_eq!(interner.resolve(STR_MIN + 9_999), None);
    assert_eq!(
        interner.resolve(42),
        None,
        "integer atoms have no dictionary entry"
    );
}

#[test]
fn a_missing_dictionary_is_not_an_error() {
    let dir = TempDir::new().expect("a temp dir");
    assert!(Dict::open(dir.path()).expect("opens").is_none());
}

#[test]
fn a_truncated_dictionary_is_reported_not_read_through() {
    let dir = TempDir::new().expect("a temp dir");
    let mut interner = Interner::open(dir.path()).expect("opens");
    interner.intern("alpha").expect("room");
    interner.intern("beta").expect("room");
    interner.flush().expect("flushes");

    let bin = dir.path().join("dict.bin");
    let data = fs::read(&bin).expect("bin");
    fs::write(&bin, data.get(..data.len() - 3).unwrap_or_default()).expect("truncate");

    let error = Dict::open(dir.path()).expect_err("a truncated dictionary is corrupt");
    let message = error.to_string();
    assert!(message.contains("corrupt"), "{message}");
    assert!(
        message.contains("rm -rf .codeintel"),
        "the message names the repair: {message}"
    );
}

#[test]
fn a_ragged_offset_table_is_reported() {
    let dir = TempDir::new().expect("a temp dir");
    let mut interner = Interner::open(dir.path()).expect("opens");
    interner.intern("alpha").expect("room");
    interner.flush().expect("flushes");

    let idx = dir.path().join("dict.idx");
    let mut data = fs::read(&idx).expect("idx");
    data.push(0);
    fs::write(&idx, data).expect("write");

    let error = Dict::open(dir.path()).expect_err("a ragged table is corrupt");
    assert!(error.to_string().contains("u64 offsets"), "{error}");
}

#[test]
fn the_dictionary_is_usable_as_the_engine_dictionary() {
    use datalog::symbols::Symbols;

    let (_dir, mut interner) = interner();
    let atom = Symbols::intern(&mut interner, "get").expect("room");
    assert_eq!(Symbols::resolve(&interner, atom), Some("get"));
}

#[test]
fn a_large_dictionary_keeps_every_id_distinct() {
    let dir = TempDir::new().expect("a temp dir");
    let mut interner = Interner::open(dir.path()).expect("opens");
    let atoms: Vec<_> = (0..5_000)
        .map(|i| interner.intern(&format!("sym{i}")).expect("room"))
        .collect();
    interner.flush().expect("flushes");

    let reopened = Interner::open(dir.path()).expect("reopens");
    for (i, atom) in atoms.iter().enumerate() {
        assert_eq!(reopened.resolve(*atom), Some(format!("sym{i}").as_str()));
    }
    assert_eq!(reopened.len(), 5_001);
}
