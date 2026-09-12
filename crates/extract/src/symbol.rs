//! Synthesizing an unresolved `SymId`.
//!
//! Tier A has no global identity to offer, so it builds one with SCIP's own
//! descriptor grammar under a `local` scheme keyed by path
//! (`specs/01-facts.md` § Symbol identity):
//!
//! ```text
//! local src/store.rs Store#get().
//! ```
//!
//! Keying by path is mandatory. SCIP's own `local N` symbols are
//! document-scoped, and two files that both contain `local 4` must not
//! collide.

/// The descriptor suffix for a kind, per the SCIP descriptor grammar.
///
/// `/` namespace, `#` type, `.` term, `().` method. A kind with no suffix here
/// is one this schema does not have, and the caller treats it as a defect in a
/// `tags.scm` rather than inventing a descriptor.
#[must_use]
pub fn suffix(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "module" => "/",
        "type" | "interface" | "struct" | "enum" | "trait" | "class" | "typealias" => "#",
        "function" | "method" | "constructor" | "macro" => "().",
        "field" | "constant" | "variable" => ".",
        _ => return None,
    })
}

/// One descriptor: the name, escaped if it needs it, plus the kind's suffix.
#[must_use]
pub fn descriptor(kind: &str, name: &str) -> Option<String> {
    Some(format!("{}{}", escape(name), suffix(kind)?))
}

/// A full unresolved symbol: the scheme, the file, and the descriptor chain
/// from outermost to innermost.
#[must_use]
pub fn symbol(path: &str, chain: &[String]) -> String {
    format!("local {path} {}", chain.concat())
}

/// SCIP's escaping: a name holding a descriptor character is backtick-quoted,
/// and a backtick inside it is doubled.
///
/// Without this, `rsplit`-ing a symbol on its suffix character corrupts any
/// name containing `#`, `.` or `/` — which is why `specs/02-extraction.md`
/// § Parent precedence insists descriptor truncation happens on the parsed
/// list rather than on the string.
#[must_use]
pub fn escape(name: &str) -> String {
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$' || c == '-' || c == '+');
    if plain {
        return name.to_string();
    }
    format!("`{}`", name.replace('`', "``"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_method_on_a_type_reads_like_scip() {
        let chain = vec![
            descriptor("struct", "Store").expect("a struct has a suffix"),
            descriptor("method", "get").expect("a method has a suffix"),
        ];
        assert_eq!(
            symbol("src/store.rs", &chain),
            "local src/store.rs Store#get()."
        );
    }

    #[test]
    fn a_module_path_nests() {
        let chain = vec![
            descriptor("module", "store").expect("suffix"),
            descriptor("constant", "LIMIT").expect("suffix"),
        ];
        assert_eq!(
            symbol("src/lib.rs", &chain),
            "local src/lib.rs store/LIMIT."
        );
    }

    #[test]
    fn a_name_holding_a_descriptor_character_is_quoted() {
        assert_eq!(escape("get"), "get");
        assert_eq!(escape("__main__"), "__main__");
        assert_eq!(escape("a.b"), "`a.b`");
        assert_eq!(escape("with space"), "`with space`");
        assert_eq!(escape("back`tick"), "`back``tick`");
        assert_eq!(escape(""), "``");
    }

    #[test]
    fn a_kind_with_no_descriptor_is_refused_not_guessed() {
        assert_eq!(suffix("unknown"), None);
        assert_eq!(descriptor("unknown", "x"), None);
    }
}
