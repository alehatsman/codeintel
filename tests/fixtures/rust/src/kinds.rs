//! Every kind our Rust `tags.scm` can emit, once each.
//!
//! This is the fixture behind the kind-fidelity test in `docs/plan.md` M2:
//! upstream's `tags.scm` maps `struct_item`, `enum_item`, `union_item` and
//! `type_item` all to `@definition.class`, so it fails this file.

/// A struct, with a documented field.
pub struct Config {
    /// How many entries to keep.
    pub limit: u32,
    quiet: bool,
}

/// An enum.
pub enum Mode {
    Fast,
    Careful,
}

/// A union. Distinct from an enum, which is the whole point.
pub union Word {
    bits: u32,
    halves: [u16; 2],
}

/// A type alias.
pub type Key = String;

/// A trait, with one required and one provided method.
pub trait Handler {
    /// Required.
    fn handle(&self, key: &Key) -> bool;

    /// Provided.
    fn name(&self) -> &'static str {
        "handler"
    }
}

/// A constant.
pub const LIMIT: u32 = 64;

/// A static.
pub static BANNER: &str = "codeintel";

impl Handler for Config {
    fn handle(&self, key: &Key) -> bool {
        self.limit > 0 && !key.is_empty()
    }
}

/// A macro.
#[macro_export]
macro_rules! shout {
    ($x:expr) => {
        format!("{}!", $x)
    };
}

/// A trait impl, so `fmt` below has a sibling with the same name.
impl std::fmt::Display for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("config")
    }
}

/// The sibling: same type, same method name, different trait.
impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("config")
    }
}

/// A trait impl on a reference, which is not the same impl as the one on the
/// type.
impl Handler for &Key {
    fn handle(&self, key: &Key) -> bool {
        !self.is_empty() && !key.is_empty()
    }
}
