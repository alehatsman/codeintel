//! `codeintel schema` — the text an agent reads to learn this system.
//!
//! The highest-leverage output in the project, and the only one with a hard
//! size budget: **~1500 tokens** (`specs/05-surface.md` § `schema`). If it does
//! not fit, the standard library is too big — cut rules, not the catalogue.
//!
//! Two things here are **generated, never declared**:
//!
//! * The rule list comes from `rules/stdlib.dl`'s `%%` doc lines, so a rule
//!   cannot ship without appearing here and a renamed one cannot go stale.
//! * The value vocabularies carry counts from *this* index. A static list
//!   advertising sixteen kinds when the index holds four is invariant 5 broken
//!   by the onboarding text itself.

use crate::census::Census;
use crate::render::{line, push};

/// The shipped rule library, for the generated rule list.
pub const STDLIB: &str = include_str!("../../../rules/stdlib.dl");

/// Every `Kind` in the closed set (`specs/01-facts.md` § Atom vocabularies),
/// in the order that spec lists them. Printed with observed counts, zeros
/// included — a kind at zero is a kind not to query.
const KINDS: [&str; 16] = [
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

/// Every `Role`.
const ROLES: [&str; 7] = [
    "def",
    "read",
    "write",
    "import",
    "test",
    "generated",
    "forward",
];

/// One advertised rule: its head as written, and its one-line doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// `innermost_at(F, Line, S)` — the head, argument names included, because
    /// argument *order* is what an agent gets wrong.
    pub head: String,
    /// The `%%` line above it.
    pub doc: String,
}

/// The advertised rules, parsed out of `stdlib.dl`.
///
/// The convention is one line per predicate, above its first clause:
///
/// ```text
/// %% within(Child, Ancestor)  transitive containment. Child is FIRST.
/// within(C, P) :- parent(C, P).
/// ```
///
/// The signature is written out rather than lifted from the rule head because
/// heads carry constants — `ref(..., "exact")`, `about(S, "sig", ...)` — and
/// the *argument names* are what an agent needs. `the_rule_list_matches_the_
/// rules` pins the two together, so a renamed or re-aritied rule fails CI
/// instead of shipping a stale catalogue.
#[must_use]
pub fn rules(stdlib: &str) -> Vec<Rule> {
    stdlib
        .lines()
        .filter_map(|line| line.strip_prefix("%%"))
        .filter_map(|text| {
            let text = text.trim();
            let close = text.find(')')?;
            let (head, doc) = text.split_at(close + 1);
            Some(Rule {
                head: head.to_string(),
                doc: doc.trim().to_string(),
            })
        })
        .collect()
}

/// The predicate name and arity of an advertised head, for the drift test.
#[must_use]
pub fn split_head(head: &str) -> Option<(&str, usize)> {
    let (name, args) = head.split_once('(')?;
    let args = args.strip_suffix(')')?;
    Some((name, args.split(',').count()))
}

/// Render the schema for this index.
#[must_use]
pub fn render(census: &Census) -> String {
    let mut out = String::new();
    let n = |count: usize| -> String { count.to_string() };

    out.push_str(
        "START HERE -- you have a location, you need a symbol\n  \
         ?- innermost_at(\"src/store.rs\", 142, S).   from ripgrep / git diff / a stack trace\n  \
         ?- def(S, F, _, N), contains(N, \"auth\").    from a word\n  \
         ?- about(S, Rel, A, B, L).                  everything about S, one round trip\n\n",
    );

    out.push_str("RELATIONS  base facts, extracted. Counts are from THIS index.\n");
    for rel in facts::RELATIONS {
        let (args, note) = signature(rel.name);
        let row = format!(
            "  {:<10} {:<44} {:<6} {note}",
            rel.name,
            args,
            n(census.rows(rel.name))
        );
        line(&mut out, row.trim_end());
    }

    out.push_str(
        "\nVALUES  closed sets, counted in THIS index. A value at 0 is a value\n        \
         you will get no rows for -- do not query it.\n",
    );
    push(&mut out, "  Kind ");
    for kind in KINDS {
        let count = census.kinds.get(kind).copied().unwrap_or(0);
        push(&mut out, &format!(" {kind} {}", n(count)));
    }
    push(&mut out, "\n  Role ");
    for role in ROLES {
        let count = census.roles.get(role).copied().unwrap_or(0);
        push(&mut out, &format!(" {role} {}", n(count)));
    }
    push(
        &mut out,
        &format!(
            "\n  Prov  exact {} (scip_ref rows)  name {} (name_ref rows)\n  Lang ",
            n(census.rows("scip_ref")),
            n(census.rows("name_ref"))
        ),
    );
    if census.langs.is_empty() {
        push(&mut out, " none indexed");
    }
    for (lang, counts) in &census.langs {
        push(&mut out, &format!(" {lang} {} files", n(counts.files)));
    }

    out.push_str("\n\nRULES  derived. A new question is a rule here, not a new verb.\n");
    for rule in rules(STDLIB) {
        let row = format!("  {:<30} {}", rule.head, rule.doc);
        line(&mut out, row.trim_end());
    }

    out.push_str(
        "\nBUILTINS  = != < <= > >= + - * / between/3 match/2 prefix/2 suffix/2\n            \
         contains/2 count{X:goal}\n\n\
         NOTES\n  \
         lines 1-based, columns 0-based UTF-8 bytes. < and > are integers only.\n  \
         integers and their strings are different atoms: Line = \"42\" never matches.\n  \
         seed impact_of/reach_of with a constant or they go all-pairs and blow\n    \
         the budget.\n  \
         a symbol column already prints `Name path:line` -- do NOT join def/at\n    \
         just to see where something is. --raw prints the SymId.\n  \
         Prov \"name\" is tree-sitter text matching: method calls x.f() are mostly\n    \
         MISSING. For precision use the _exact rules, and run `codeintel status`\n    \
         to see whether a SCIP index is present and fresh.\n\n\
         EXAMPLES\n  \
         what does this diff hunk affect?\n  \
         ?- innermost_at(\"src/store.rs\", 142, S), impact_of(S, C),\n     \
         def(C, F, _, N), !is_test(F).\n\n  \
         no module under ui/ may import from db/ -- needs no SCIP index\n  \
         ?- import(F, M, _), prefix(F, \"src/ui/\"), contains(M, \"db\").\n\n  \
         exported and unreferenced outside its own file\n  \
         ?- dead_export(S), def(S, F, _, _), prefix(F, \"src/\").\n",
    );

    if !census.indexed {
        out.push_str(
            "\nTHERE IS NO INDEX HERE. Every count above is 0 because nothing has been\n\
             indexed, not because your code has none of these. run: codeintel index .\n",
        );
    }
    out
}

/// The argument names for a base relation, as `specs/01-facts.md` writes them.
///
/// Hand-written because the names carry the meaning and `facts::RELATIONS` only
/// knows arities. A drift test pins the two together.
fn signature(name: &str) -> (&'static str, &'static str) {
    match name {
        "file" => ("(F, Lang)", ""),
        "def" => ("(S, F, Kind, Name)", ""),
        "def_span" => ("(S, StartLine, EndLine, StartByte, EndByte)", ""),
        "def_name" => ("(S, Line, Col)", "the identifier token"),
        "def_sig" => ("(S, Sig)", ""),
        "def_doc" => ("(S, Doc)", ""),
        "parent" => ("(Child, Parent)", "one level; see within/2"),
        "exported" => ("(S)", ""),
        "resolved" => ("(S)", "a SCIP symbol anchored to this def"),
        "import" => ("(F, Module, Alias)", ""),
        "scip_ref" => ("(S, F, Line, Col, From, Role)", "compiler-resolved"),
        "name_ref" => ("(Name, F, Line, Col, From)", "unresolved, tier A"),
        "implements" => ("(S, T, Prov)", ""),
        "extern" => ("(S, Manager, Package, Version)", ""),
        _ => ("", ""),
    }
}
