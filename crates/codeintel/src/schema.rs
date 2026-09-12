//! `codeintel schema` — the text an agent reads to learn this system.
//!
//! The highest-leverage output in the project, and the only one with a hard
//! size budget: **~1500 tokens** (`specs/05-surface.md` § `schema`). If it does
//! not fit, the standard library is too big — cut rules, not the catalogue.
//!
//! Nothing here is declared twice. The rule list comes from `rules/stdlib.dl`,
//! the kinds and roles from `extract::lang`, the relations from
//! `facts::RELATIONS`, and the counts from the index in front of us. A
//! catalogue that restates any of those is a catalogue that can disagree with
//! them, and `schema` disagreeing with the index is invariant 5 broken by the
//! onboarding text itself.

use std::fmt;

use crate::census::Census;

/// The shipped rule library, for the generated rule list.
pub const STDLIB: &str = include_str!("../../../rules/stdlib.dl");

/// One advertised rule: its head as written, and its doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// `innermost_at(F, Line, S)` — the head, argument names included, because
    /// argument *order* is what an agent gets wrong.
    pub head: String,
    /// The `%%` lines above it, joined.
    pub doc: String,
}

/// The advertised rules, parsed out of `stdlib.dl`.
///
/// The convention is a `%%` block above a predicate's first clause. The first
/// line carries the signature; any further `%%` lines continue the doc:
///
/// ```text
/// %% about(S, Rel, A, B, L)  all of S in one trip. Rel = sig|doc|defined|
/// %%   caller|callee|implements|implementor|test|extern
/// about(S, "sig", Sig, "", 0) :- def_sig(S, Sig).
/// ```
///
/// Continuation lines are **joined, not dropped**. A parser that demanded a
/// signature on every `%%` line silently truncated `about`'s value list at the
/// wrap, so the one relation designed to answer everything in one round trip
/// advertised half of what it answers.
///
/// The signature is written out rather than lifted from the clause because
/// heads carry constants — `ref(..., "exact")`, `about(S, "sig", ...)` — and
/// the argument names are what an agent needs.
/// `the_rule_list_matches_the_rules` pins the two together.
#[must_use]
pub fn rules(stdlib: &str) -> Vec<Rule> {
    let mut out: Vec<Rule> = Vec::new();
    for line in stdlib.lines() {
        let Some(text) = line.strip_prefix("%%") else {
            continue;
        };
        let text = text.trim();
        if let Some(close) = text.find(')') {
            let (head, doc) = text.split_at(close + 1);
            out.push(Rule {
                head: head.to_string(),
                doc: doc.trim().to_string(),
            });
        } else if let Some(rule) = out.last_mut() {
            // No signature on this line: a continuation of the block above it.
            rule.doc.push(' ');
            rule.doc.push_str(text);
        }
    }
    out
}

/// The predicate name and arity of an advertised head, for the drift test.
#[must_use]
pub fn split_head(head: &str) -> Option<(&str, usize)> {
    let (name, args) = head.split_once('(')?;
    let args = args.strip_suffix(')')?;
    Some((name, args.split(',').count()))
}

/// The schema for one index.
///
/// A `Display` rather than a function returning `String`: the whole output is
/// one formatting operation, every `write!` carries its own error, and a caller
/// that wants a `String` asks for `.to_string()`.
#[derive(Debug, Clone, Copy)]
pub struct Schema<'a> {
    /// The counts to print against the catalogue.
    pub census: &'a Census,
}

impl fmt::Display for Schema<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let census = self.census;
        f.write_str(
            "START HERE -- you have a location, you need a symbol\n  \
             ?- innermost_at(\"src/store.rs\", 142, S).   from ripgrep / git diff / a stack trace\n  \
             ?- def(S, F, _, N), contains(N, \"auth\").    from a word\n  \
             ?- about(S, Rel, A, B, L).                  everything about S, one round trip\n\n\
             RELATIONS  base facts, extracted. Counts are from THIS index.\n",
        )?;
        for rel in facts::RELATIONS {
            let (args, note) = signature(rel.name);
            let row = format!(
                "  {:<10} {:<44} {:<6} {note}",
                rel.name,
                args,
                census.rows(rel.name)
            );
            writeln!(f, "{}", row.trim_end())?;
        }

        f.write_str(
            "\nVALUES  closed sets, counted in THIS index. A value at 0 is a value\n        \
             you will get no rows for -- do not query it.\n  Kind ",
        )?;
        for kind in extract::lang::KINDS {
            write!(f, " {kind} {}", census.kinds.get(*kind).unwrap_or(&0))?;
        }
        f.write_str("\n  Role ")?;
        for role in extract::lang::ROLES {
            write!(f, " {role} {}", census.roles.get(*role).unwrap_or(&0))?;
        }
        write!(
            f,
            "\n  Prov  exact {} (scip_ref rows)  name {} (name_ref rows)\n  Lang ",
            census.rows("scip_ref"),
            census.rows("name_ref")
        )?;
        if census.langs.is_empty() {
            f.write_str(" none indexed")?;
        }
        for (lang, counts) in &census.langs {
            write!(f, " {lang} {} files", counts.files)?;
        }

        f.write_str("\n\nRULES  derived. A new question is a rule here, not a new verb.\n")?;
        for rule in rules(STDLIB) {
            let row = format!("  {:<30} {}", rule.head, rule.doc);
            writeln!(f, "{}", row.trim_end())?;
        }

        f.write_str(
            "\nBUILTINS  = != < <= > >= + - * / between/3 match/2 prefix/2 suffix/2\n            \
             contains/2 count{X:goal}\n\n\
             NOTES\n  \
             lines 1-based, columns 0-based UTF-8 bytes. < and > are integers only.\n  \
             integers and their strings are different atoms: Line = \"42\" never matches.\n  \
             seed impact_of/reach_of with a constant or they go all-pairs.\n  \
             a symbol column already prints `Name path:line` -- do NOT join def/at\n    \
             to see where something is. --raw prints the SymId.\n  \
             Prov \"name\" is text matching: method calls x.f() are mostly MISSING.\n    \
             For precision use the _exact rules; `codeintel status` says whether\n    \
             a SCIP index is present and fresh.\n\n\
             EXAMPLES  more in docs/cookbook.md, including conformance rules\n  \
             what does this diff hunk affect?\n  \
             ?- innermost_at(\"src/store.rs\", 142, S), impact_of(S, C),\n     \
             def(C, F, _, N), !is_test(F).\n\n  \
             no module under ui/ may import from db/ -- needs no SCIP index\n  \
             ?- import(F, M, _), prefix(F, \"src/ui/\"), contains(M, \"db\").\n",
        )?;

        if !census.indexed {
            f.write_str(
                "\nTHERE IS NO INDEX HERE. Every count above is 0 because nothing has been\n\
                 indexed, not because your code has none of these. run: codeintel index .\n",
            )?;
        }
        Ok(())
    }
}

/// The argument names for a base relation, as `specs/01-facts.md` writes them,
/// and an optional note.
///
/// Hand-written because the names carry the meaning and `facts::RELATIONS` only
/// knows arities. `every_relation_has_a_signature` asserts there is an entry
/// for every relation and that its comma count matches the declared arity, so
/// adding a relation cannot ship a blank row in the project's most-read output.
#[must_use]
pub fn signature(name: &str) -> (&'static str, &'static str) {
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
