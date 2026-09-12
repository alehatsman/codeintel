//! A second connection layer, exporting a function with the same name as
//! `db::conn::open`. That ambiguity is the whole point: tier A cannot decide
//! which `open` a caller in a third file means, and gives up rather than
//! guessing. A compiler can.

/// Open a socket.
pub fn open(url: &str) -> bool {
    url.starts_with("tcp://")
}
