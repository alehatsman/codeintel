//! A caller in a third file. `open` is exported twice repo-wide, so tier A's
//! `!ambiguous(N)` guard refuses the edge and `calls` has nothing here. Tier B
//! resolves it exactly.

use crate::db::conn;

/// Start up.
pub fn start() -> bool {
    conn::open("sqlite://memory")
}
