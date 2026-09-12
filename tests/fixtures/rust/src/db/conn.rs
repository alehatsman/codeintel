//! The database layer. Nothing here may import from `ui/`.

/// Open a connection.
pub fn open(url: &str) -> bool {
    !url.is_empty()
}
