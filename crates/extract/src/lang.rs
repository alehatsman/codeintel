//! The per-language table. One row per language, and the row is data.
//!
//! `specs/02-extraction.md`: adding a language is an authored `tags.scm`, an
//! authored `imports.scm`, and a row here. **If you find yourself writing
//! per-language Rust, the extractor is wrong** — fix the extractor. Everything
//! that varies between languages appears in [`Lang`] as a value.

use tree_sitter::Language;

/// The closed `Kind` vocabulary, from `specs/01-facts.md` § Atom vocabularies.
///
/// A `@definition.<suffix>` capture whose suffix is not here after
/// [`Lang::kind`] has remapped it is a defect in that language's `tags.scm`,
/// and the extractor reports it rather than emitting `unknown` — `unknown` is
/// for a construct we genuinely cannot classify, not for a typo.
pub const KINDS: &[&str] = &[
    "module",
    "type",
    "interface",
    "struct",
    "enum",
    "trait",
    "class",
    "function",
    "method",
    "constructor",
    "field",
    "constant",
    "variable",
    "macro",
    "typealias",
    "unknown",
];

/// Every `Role` an occurrence can carry (`specs/01-facts.md` § Atom
/// vocabularies), in the order that spec lists them.
///
/// The vocabulary lives here beside [`KINDS`] so there is one list to read and
/// one list to print. `scip.rs` maps SCIP's role bits onto these names and a
/// test there asserts it produces nothing outside this set.
pub const ROLES: &[&str] = &[
    "def",
    "read",
    "write",
    "import",
    "test",
    "generated",
    "forward",
];

/// Kinds that own methods: a `function` defined inside one becomes a `method`.
///
/// Shared across every language rather than decided in each `tags.scm`, because
/// a query cannot see its own nesting without double-capturing the node — which
/// is exactly the upstream defect that emits two `def` rows for every Rust
/// method.
pub const TYPE_LIKE: &[&str] = &["struct", "enum", "trait", "class", "interface", "type"];

/// How a language says "visible outside the defining module".
///
/// Best-effort by specification (`specs/01-facts.md` § `exported`): absence is
/// not proof of privacy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Export {
    /// The definition node has a direct child of this kind. Rust `pub`.
    ChildKind(&'static str),
    /// The name starts with an upper-case letter. Go.
    Capitalized,
    /// The name does not start with `_`. Python convention.
    NotUnderscored,
}

/// One language.
#[derive(Debug, Clone, Copy)]
pub struct Lang {
    /// As it appears in `file(F, Lang)`.
    pub name: &'static str,
    /// File extensions, without the dot.
    pub extensions: &'static [&'static str],
    /// The grammar.
    pub language: fn() -> Language,
    /// Our authored definition/scope/reference query.
    pub tags: &'static str,
    /// Our authored import query.
    pub imports: &'static str,
    /// Capture-suffix overrides, applied before [`KINDS`] is checked.
    pub kind_remap: &'static [(&'static str, &'static str)],
    /// The export predicate.
    pub export: Export,
    /// Comment prefixes that count as documentation.
    pub doc_markers: &'static [&'static str],
    /// Node kinds that are comments.
    pub comment_kinds: &'static [&'static str],
    /// Node kinds that decorate the following item and belong to its span.
    ///
    /// In `def_span`, not in `def_sig`: a Rust `#[derive(Debug)]` is part of
    /// the definition and is not part of how you would say its name.
    pub attribute_kinds: &'static [&'static str],
    /// Node kinds that are the *head* of the following item's declaration, and
    /// so belong to its span **and** its signature.
    ///
    /// Go needs this and Rust does not. A Go definition is the spec node —
    /// `const_spec`, `type_spec` — so that a grouped `const ( A = 1 \n B = 2 )`
    /// yields two definitions with two spans rather than two sharing one, which
    /// leaves `const` a preceding sibling. It is not an attribute: without it
    /// `def_sig` for `const Limit = 64` reads `Limit`, which names the thing
    /// without saying what it is.
    pub keyword_kinds: &'static [&'static str],
    /// Characters that end a declaration header, for `def_sig`.
    pub sig_stops: &'static [char],
    /// The canonical SCIP indexer command, verbatim and copy-pasteable.
    ///
    /// `index` prints it for every language it found. Not "no SCIP index
    /// found" — the literal line, because the gap between tier A and tier B is
    /// the difference between a symbol map and a call graph and a user must
    /// never have to go looking for how to close it
    /// (`specs/02-extraction.md` § Acquisition). It is **not** part of
    /// [`crate::fingerprint`]: it changes no fact.
    pub indexer: &'static str,
}

impl Lang {
    /// The `Kind` for a `@definition.<suffix>` capture, or `None` when the
    /// suffix is not a kind this schema has.
    #[must_use]
    pub fn kind(&self, suffix: &str) -> Option<&'static str> {
        let mapped = self
            .kind_remap
            .iter()
            .find_map(|(from, to)| (*from == suffix).then_some(*to))
            .unwrap_or(suffix);
        KINDS.iter().copied().find(|k| *k == mapped)
    }
}

/// Every registered language.
pub const LANGS: &[Lang] = &[
    Lang {
        name: "go",
        extensions: &["go"],
        language: || tree_sitter_go::LANGUAGE.into(),
        tags: include_str!("../../../queries/go/tags.scm"),
        imports: include_str!("../../../queries/go/imports.scm"),
        // Every capture suffix in `go/tags.scm` is already a Kind. The
        // discrimination upstream folds into one `@definition.type` happens in
        // the query, where the grammar can see *which* type node it is, rather
        // than here, where it could only be guessed at.
        kind_remap: &[],
        export: Export::Capitalized,
        // Go has no distinguished doc-comment syntax: a doc comment is an
        // ordinary comment immediately preceding the declaration. That is the
        // language's own rule — `go doc` reads exactly this — not an inference.
        doc_markers: &["//", "/*"],
        comment_kinds: &["comment"],
        // Go has no attributes.
        attribute_kinds: &[],
        keyword_kinds: &["const", "var", "type"],
        sig_stops: &['{', '='],
        indexer: "scip-go",
    },
    Lang {
        name: "rust",
        extensions: &["rs"],
        language: || tree_sitter_rust::LANGUAGE.into(),
        tags: include_str!("../../../queries/rust/tags.scm"),
        imports: include_str!("../../../queries/rust/imports.scm"),
        // A Rust `union` is a distinct construct and `union` is not in the
        // closed Kind set, so it maps to `type` — distinct from `enum`, which
        // is what docs/plan.md M2's kind-fidelity test is for.
        // specs/02-extraction.md's SCIP table maps `Union` the same way so the
        // two tiers agree.
        kind_remap: &[("union", "type")],
        export: Export::ChildKind("visibility_modifier"),
        // `//!` is deliberately absent. It is an *inner* doc comment — it
        // documents the enclosing module, not the item that follows it — so
        // treating it as a marker attaches a file header to whatever definition
        // happens to come first. Tier A emits no module-level doc; nothing else
        // would be honest.
        doc_markers: &["///", "/**"],
        comment_kinds: &["line_comment", "block_comment"],
        attribute_kinds: &["attribute_item", "inner_attribute_item"],
        // A Rust definition node already contains its own head.
        keyword_kinds: &[],
        sig_stops: &['{', ';', '='],
        indexer: "rust-analyzer scip .",
    },
];

/// The language for a path, by extension.
#[must_use]
pub fn for_path(path: &str) -> Option<&'static Lang> {
    let ext = path.rsplit_once('.')?.1;
    LANGS.iter().find(|l| l.extensions.contains(&ext))
}

/// The language of that name.
#[must_use]
pub fn by_name(name: &str) -> Option<&'static Lang> {
    LANGS.iter().find(|l| l.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_finds_its_language() {
        assert_eq!(for_path("src/store.rs").map(|l| l.name), Some("rust"));
        assert_eq!(for_path("src/store.RS").map(|l| l.name), None);
        assert_eq!(for_path("Makefile").map(|l| l.name), None);
        assert_eq!(for_path("app/main.py").map(|l| l.name), None);
    }

    #[test]
    fn the_remap_is_the_only_thing_between_a_capture_and_a_kind() {
        let rust = by_name("rust").expect("rust is registered");
        assert_eq!(rust.kind("struct"), Some("struct"));
        assert_eq!(rust.kind("union"), Some("type"));
        assert_eq!(rust.kind("class"), Some("class"));
        assert_eq!(rust.kind("impl"), None);
    }

    #[test]
    fn every_grammar_loads() {
        // The ABI-drift smoke test research.md § Language coverage asks for:
        // the grammar crates span tree-sitter 0.23.x to 0.25.x against a 0.27
        // runtime, and a mismatch shows up here rather than as zero rows.
        for lang in LANGS {
            let mut parser = tree_sitter::Parser::new();
            parser
                .set_language(&(lang.language)())
                .unwrap_or_else(|e| panic!("{}: {e}", lang.name));
        }
    }

    #[test]
    fn every_query_compiles_against_its_grammar() {
        for lang in LANGS {
            let language = (lang.language)();
            for (what, source) in [("tags", lang.tags), ("imports", lang.imports)] {
                tree_sitter::Query::new(&language, source)
                    .unwrap_or_else(|e| panic!("{}/{what}.scm: {e}", lang.name));
            }
        }
    }
}
