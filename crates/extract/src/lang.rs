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
    // `UnspecifiedSymbolRole`: the indexer left the bitset empty.
    "unknown",
];

/// Kinds that own methods: a `function` defined inside one becomes a `method`.
///
/// Shared across every language rather than decided in each `tags.scm`, because
/// a query cannot see its own nesting without double-capturing the node — which
/// is exactly the upstream defect that emits two `def` rows for every Rust
/// method.
pub const TYPE_LIKE: &[&str] = &["struct", "enum", "trait", "class", "interface", "type"];

/// How a language *states* visibility, which is not the same question as
/// whether a symbol is reachable from outside the crate.
///
/// This decides `visibility(S, Vis)` only. `exported` is a rule over `parent`
/// in `rules/stdlib.dl` (`specs/01-facts.md` § Derived relations); an extractor
/// that walked ancestors to answer it here would be inferring, which invariant
/// 1 forbids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Export {
    /// The definition node has a direct child of this kind whose text is
    /// exactly one of `words`.
    ///
    /// Rust: a `visibility_modifier` reading `pub`. It must be the *text* and
    /// not merely the node, because `pub(crate)` is also a `visibility_modifier`
    /// and states a bounded visibility, which is `restricted`.
    ChildWord {
        /// The child node kind that carries the modifier.
        kind: &'static str,
        /// The exact texts of that child which mean unrestricted visibility.
        words: &'static [&'static str],
    },
    /// The name starts with an upper-case letter. Go.
    Capitalized,
    /// The name does not start with `_`, or starts **and** ends with `__`.
    ///
    /// Python. PEP 8: a single leading underscore is a "weak internal use
    /// indicator", and a dunder (`__init__`) is a magic name, not a private
    /// one. Both are statements the name itself makes; neither is a walk.
    NotUnderscored,
}

/// What a definition states about its own visibility.
///
/// The `Vis` vocabulary of `specs/01-facts.md` § Atom vocabularies. Three
/// values, and the third is a statement about the *grammar* rather than about
/// the symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vis {
    /// States unrestricted visibility outside its module.
    Public,
    /// States a bounded one, or states none where the grammar allows one.
    Restricted,
    /// The grammar gives this node kind no slot to state one, so the answer
    /// comes from the enclosing definition — in the rule layer, not here.
    Inherited,
}

impl Vis {
    /// As it appears in `visibility(S, Vis)`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Restricted => "restricted",
            Self::Inherited => "inherited",
        }
    }
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
    /// The visibility predicate: how a node states `public` vs `restricted`.
    pub export: Export,
    /// [`Kind`s](KINDS) of an owner whose members cannot state a visibility, so
    /// theirs is `inherited` and the rule layer resolves it through `parent`.
    ///
    /// Keyed on the **owner**, not on the member's node kind, because the node
    /// kind does not decide it: a provided trait method and a free function are
    /// both `function_item`, and only the first is forbidden a `pub`. Keying on
    /// the owner also collapses what would otherwise be a list of node kinds
    /// per language into the language-neutral `Kind` vocabulary.
    ///
    /// Rust: `enum` (a variant cannot say `pub`) and `trait` (nor can a trait
    /// item, required or provided). `struct` is deliberately absent — a Rust
    /// field *can* say `pub`, so one that does not has stated privacy.
    /// Trait `impl` blocks are handled by their own `@scope.impl` capture,
    /// since an inherent `impl` owns members that may say `pub` and carries the
    /// same `Kind`.
    pub vis_inherits_under: &'static [&'static str],
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
        // Go decides visibility from the identifier, and every declaration has
        // one — an interface method included — so nothing inherits.
        vis_inherits_under: &[],
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
        name: "python",
        extensions: &["py"],
        language: || tree_sitter_python::LANGUAGE.into(),
        tags: include_str!("../../../queries/python/tags.scm"),
        imports: include_str!("../../../queries/python/imports.scm"),
        // Every capture suffix in `python/tags.scm` is already a Kind. A
        // method is a `function` the extractor promotes under its class.
        kind_remap: &[],
        export: Export::NotUnderscored,
        // Every Python definition has a name to read, so nothing inherits.
        vis_inherits_under: &[],
        // Python documents with docstrings, not comments. They sit INSIDE the
        // definition, where the preamble walk cannot see them, so `tags.scm`
        // names them with `@doc` and no comment marker counts as one.
        doc_markers: &[],
        comment_kinds: &["comment"],
        // A decorator precedes the `function_definition` inside the
        // `decorated_definition` that wraps both; it is part of the
        // definition and not part of how you would say its name.
        attribute_kinds: &["decorator"],
        // A Python definition node already contains its own head.
        keyword_kinds: &[],
        // `def f(x: int) -> T:` and `class C(B):` end at the colon; `x = 1`
        // at the `=`. A colon or `=` inside the parameter list is nested and
        // does not count.
        sig_stops: &[':', '='],
        indexer: "scip-python index . --project-name <name>",
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
        // `impl` is the `@scope.impl` capture: still a type-like owner for
        // descriptor synthesis, exactly as `@scope.type` is, but a distinct
        // capture so the extractor can tell a trait impl from an inherent one.
        kind_remap: &[("union", "type"), ("impl", "type")],
        // `pub` only. `pub(crate)`, `pub(super)` and `pub(in path)` are all
        // `visibility_modifier` nodes too, and all state a visibility bounded
        // by the crate — which is the boundary `exported` asks about, so they
        // are `restricted`. Matching the node rather than its text was the bug
        // in #13.
        export: Export::ChildWord {
            kind: "visibility_modifier",
            words: &["pub"],
        },
        // A variant, and any trait item, have nowhere to write `pub`; they are
        // as visible as the enum or trait that owns them, which `stdlib.dl`
        // resolves through `parent`. `struct` is not here: Rust lets a field
        // say `pub`.
        vis_inherits_under: &["enum", "trait"],
        // `//!` is deliberately absent. It is an *inner* doc comment — it
        // documents the enclosing module, not the item that follows it — so
        // treating it as a marker attaches a file header to whatever definition
        // happens to come first. Tier A emits no module-level doc; nothing else
        // would be honest.
        doc_markers: &["///", "/**"],
        comment_kinds: &["line_comment", "block_comment"],
        // Outer attributes only. `#![allow(...)]` belongs to the file or module
        // that encloses it, not to the item written under it, so walking back
        // over one puts a crate attribute inside the first definition's span
        // and an edit built on that span deletes it.
        attribute_kinds: &["attribute_item"],
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
        assert_eq!(for_path("app/main.py").map(|l| l.name), Some("python"));
    }

    #[test]
    fn the_remap_is_the_only_thing_between_a_capture_and_a_kind() {
        let rust = by_name("rust").expect("rust is registered");
        assert_eq!(rust.kind("struct"), Some("struct"));
        assert_eq!(rust.kind("union"), Some("type"));
        assert_eq!(rust.kind("class"), Some("class"));
        // `@scope.impl` is a type-like owner for descriptor synthesis, exactly
        // as `@scope.type` is; the separate capture exists so the extractor can
        // tell a trait impl from an inherent one, not to make a new Kind.
        assert_eq!(rust.kind("impl"), Some("type"));
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
