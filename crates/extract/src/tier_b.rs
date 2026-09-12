//! Tier B: SCIP facts, and the join that unifies identity with tier A.
//!
//! Two halves. [`Anchors`] is what tier A consults while it emits — the
//! identity, owner, doc and signature SCIP has for a definition at a given
//! identifier position — so that a tier-A fact lands on the resolved symbol
//! **as it is written**, and nothing is ever rewritten twice. [`emit`] is
//! everything tier A has no counterpart for: references, implementations,
//! packages, and definitions in files tier A never parsed.
//!
//! Matching is on the *identifier* position, not the full span. The two tiers
//! disagree about whether a span includes decorators, attributes and doc
//! comments; they always agree on where the name token starts
//! (`specs/02-extraction.md` § The anchor join).

use std::collections::{BTreeMap, BTreeSet};

use facts::{Interner, Segment};

use crate::error::{Error, Result};
use crate::scip::Ingest;
use crate::sweep::{self, Span};

/// What SCIP knows about the definition whose name token starts here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// The SCIP symbol. The tier-A definition adopts it as its `SymId`.
    pub symbol: String,
    /// The semantic owner, by the precedence in
    /// `specs/02-extraction.md` § Parent precedence. `None` falls back to tier
    /// A's span nesting.
    pub parent: Option<String>,
    /// `documentation`, used only when tier A found none.
    pub doc: Option<String>,
    /// `signature_documentation.text`, used only when tier A found none.
    pub sig: Option<String>,
}

/// One file's anchors, keyed by `(line, col)` of the identifier token.
#[derive(Debug, Clone, Default)]
pub struct Anchors {
    at: BTreeMap<(u32, u32), Anchor>,
}

impl Anchors {
    /// Build the anchor index for one file.
    ///
    /// `known` is every symbol this SCIP index defines anywhere; parent
    /// precedence rule 1 only fires when the truncated descriptor actually
    /// names one of them.
    #[must_use]
    pub fn of(ingest: &Ingest, path: &str) -> Self {
        let known = defined(ingest);
        let Some(doc) = ingest.docs.get(path) else {
            return Self::default();
        };
        let mut at = BTreeMap::new();
        for def in &doc.defs {
            // Defined elsewhere too: not an identity, a collision. Tier A keeps
            // its own symbol for this definition.
            if ingest.collisions.contains(&def.symbol) {
                continue;
            }
            let info = ingest.symbols.get(&def.symbol);
            at.insert(
                (def.line, def.col),
                Anchor {
                    parent: parent_of(&def.symbol, info, &known),
                    symbol: def.symbol.clone(),
                    doc: info.and_then(|i| i.doc.clone()),
                    sig: info.and_then(|i| i.sig.clone()),
                },
            );
        }
        Self { at }
    }

    /// The anchor for an identifier token at `line:col`, if SCIP saw one.
    #[must_use]
    pub fn at(&self, line: u32, col: u32) -> Option<&Anchor> {
        self.at.get(&(line, col))
    }

    /// How many anchors this file has. `index` reports the anchor rate from it.
    #[must_use]
    pub fn len(&self) -> usize {
        self.at.len()
    }

    /// True when SCIP covered nothing in this file.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }
}

/// How many facts tier B produced for one file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// `scip_ref` rows.
    pub refs: usize,
    /// `resolved` rows — definitions a compiler agreed exist.
    pub resolved: usize,
    /// `def` rows for definitions tier A had no counterpart for.
    pub only: usize,
    /// Definition occurrences whose symbol this index defines in more than
    /// one document. Not adopted, not emitted; counted so `status` can say
    /// so (`specs/02-extraction.md` § The anchor join).
    pub collided: usize,
}

/// Emit every tier-B fact for one file into its segment.
///
/// `anchored` is the byte span and final symbol of each tier-A definition in
/// this file, which is what attributes a reference to the function it sits
/// inside — the same sweep tier A uses, called a second time with references
/// as zero-width spans (`specs/02-extraction.md` § Deriving `From`).
///
/// # Errors
/// An exhausted dictionary, or a position past the integer-atom range.
pub fn emit(
    seg: &mut Segment,
    ingest: &Ingest,
    path: &str,
    src: Option<&str>,
    anchored: &[(Span, String)],
    interner: &mut Interner,
) -> Result<Counts> {
    let Some(doc) = ingest.docs.get(path) else {
        return Ok(Counts::default());
    };
    let file = atom(interner, path)?;
    let lines = LineIndex::of(src.unwrap_or_default());
    let mut counts = Counts::default();

    // Definitions, in span order, so the sweep sees a stable input. Tier A's
    // spans where it parsed the file; SCIP's `typed_enclosing_range` where it
    // did not.
    let mut owners: Vec<(Span, String)> = anchored.to_vec();
    let anchors = Anchors::of(ingest, path);
    let known = defined(ingest);
    for def in &doc.defs {
        if ingest.collisions.contains(&def.symbol) {
            counts.collided += 1;
            continue;
        }
        let resolved = atom(interner, &def.symbol)?;
        if anchors.at(def.line, def.col).is_some() && anchored.iter().any(|(_, s)| *s == def.symbol)
        {
            // Tier A owns the `def` row; it already adopted this symbol.
            push(seg, "resolved", &[resolved]);
            counts.resolved += 1;
            continue;
        }
        counts.only += 1;
        counts.resolved += 1;
        push(seg, "resolved", &[resolved]);
        let info = ingest.symbols.get(&def.symbol);
        let kind = info.map_or("unknown", |i| i.kind);
        let name = info.map_or("", |i| i.name.as_str());
        push(
            seg,
            "def",
            &[resolved, file, atom(interner, kind)?, atom(interner, name)?],
        );
        // No span, no signature: SCIP's ranges are not our `def_span`, which is
        // the *full* definition including attributes and docs. Emitting an
        // approximate one would put an edit in the wrong place.
        if let Some(doc_text) = info.and_then(|i| i.doc.as_deref()) {
            push(seg, "def_doc", &[resolved, atom(interner, doc_text)?]);
        }
        if let Some(sig) = info.and_then(|i| i.sig.as_deref()) {
            push(seg, "def_sig", &[resolved, atom(interner, sig)?]);
        }
        let owner = match parent_of(&def.symbol, info, &known) {
            Some(parent) => atom(interner, &parent)?,
            None => file,
        };
        push(seg, "parent", &[resolved, owner]);
        if let Some((sl, sc, el, ec)) = def.enclosing {
            owners.push(((lines.byte(sl, sc), lines.byte(el, ec)), def.symbol.clone()));
        }
    }

    // `scip_impl` and `extern` are keyed by symbol, and a symbol is defined in
    // exactly one file, so emitting them beside that definition keeps every row
    // a function of its own file.
    //
    // No provenance column: everything here came from the indexer, so it is
    // `exact` by construction exactly as `scip_ref` is. `implements(S, T,
    // "exact")` is the one-line rule over it in `stdlib.dl`.
    for def in &doc.defs {
        if ingest.collisions.contains(&def.symbol) {
            continue;
        }
        let s = atom(interner, &def.symbol)?;
        for target in ingest
            .symbols
            .get(&def.symbol)
            .map(|i| i.implements.as_slice())
            .unwrap_or_default()
        {
            push(seg, "scip_impl", &[s, atom(interner, target)?]);
        }
    }

    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for occurrence in &doc.refs {
        if let Some((manager, package, version)) = ingest.externs.get(&occurrence.symbol)
            && seen.insert(occurrence.symbol.as_str())
        {
            push(
                seg,
                "extern",
                &[
                    atom(interner, &occurrence.symbol)?,
                    atom(interner, manager)?,
                    atom(interner, package)?,
                    atom(interner, version)?,
                ],
            );
        }
    }

    // One sweep, references as zero-width spans, exactly as tier A does it.
    let mut spans: Vec<Span> = owners.iter().map(|(span, _)| *span).collect();
    let positions: Vec<usize> = doc.refs.iter().map(|r| lines.byte(r.line, r.col)).collect();
    spans.extend(positions.iter().map(|at| (*at, *at)));
    let parents = sweep::containment(&spans);

    for (k, occurrence) in doc.refs.iter().enumerate() {
        let from = match parents
            .get(owners.len() + k)
            .copied()
            .flatten()
            .and_then(|j| owners.get(j))
        {
            Some((_, symbol)) => atom(interner, symbol)?,
            None => file,
        };
        counts.refs += 1;
        push(
            seg,
            "scip_ref",
            &[
                atom(interner, &occurrence.symbol)?,
                file,
                int(path, occurrence.line)?,
                int(path, occurrence.col)?,
                from,
                atom(interner, occurrence.role)?,
            ],
        );
    }
    Ok(counts)
}

/// Every symbol this index defines, anywhere.
fn defined(ingest: &Ingest) -> BTreeSet<&str> {
    ingest
        .docs
        .values()
        .flat_map(|d| d.defs.iter().map(|def| def.symbol.as_str()))
        .collect()
}

/// The semantic owner of a symbol: descriptor prefix, else `enclosing_symbol`,
/// else nothing — and "nothing" means tier A's span nesting stands.
///
/// The order is fixed by `specs/02-extraction.md` § Parent precedence. Rule 1
/// is preferred because the descriptor grammar is mandatory, so every indexer
/// supplies it; it only fires when the truncated symbol actually names a
/// definition, or a Go method would be owned by a symbol nothing defines.
fn parent_of(
    symbol: &str,
    info: Option<&crate::scip::Info>,
    known: &BTreeSet<&str>,
) -> Option<String> {
    if let Some(owner) = crate::scip::owner(symbol)
        && known.contains(owner.as_str())
    {
        return Some(owner);
    }
    info.and_then(|i| i.enclosing.clone())
        .filter(|e| known.contains(e.as_str()))
}

/// Byte offsets of each line start, so a `(line, col)` becomes a byte position
/// the shared sweep can compare against tier A's spans.
#[derive(Debug, Default)]
struct LineIndex {
    starts: Vec<usize>,
    len: usize,
}

impl LineIndex {
    fn of(src: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(src.match_indices('\n').map(|(i, _)| i + 1));
        Self {
            starts,
            len: src.len(),
        }
    }

    /// The byte offset of `line:col`, 1-based line and 0-based byte column.
    ///
    /// Out of range clamps to the end of the file rather than wrapping to zero:
    /// a position past the end means the source moved under the SCIP index, and
    /// attributing that reference to the first definition in the file would be
    /// a confident lie.
    fn byte(&self, line: u32, col: u32) -> usize {
        let index = usize::try_from(line).unwrap_or(0).saturating_sub(1);
        let start = self.starts.get(index).copied().unwrap_or(self.len);
        start
            .saturating_add(usize::try_from(col).unwrap_or(0))
            .min(self.len)
    }
}

fn push(seg: &mut Segment, relation: &str, row: &[u32]) {
    let accepted = seg.push(relation, row);
    debug_assert!(accepted, "{relation} row {row:?} refused");
}

fn atom(interner: &mut Interner, s: &str) -> Result<u32> {
    interner.intern(s).ok_or(Error::Atoms)
}

fn int(path: &str, n: u32) -> Result<u32> {
    datalog::atom::int_atom(i64::from(n)).ok_or_else(|| Error::TooLarge {
        path: path.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scip::{Def, Doc, Info, Ref};

    fn ingest() -> Ingest {
        let mut ingest = Ingest::default();
        ingest.docs.insert(
            "src/store.rs".to_string(),
            Doc {
                lang: "rust".to_string(),
                defs: vec![
                    Def {
                        symbol: "sc cargo p 1.0 store/Store#".to_string(),
                        line: 1,
                        col: 11,
                        enclosing: Some((1, 0, 9, 1)),
                    },
                    Def {
                        symbol: "sc cargo p 1.0 store/Store#get().".to_string(),
                        line: 4,
                        col: 7,
                        enclosing: Some((4, 4, 6, 5)),
                    },
                ],
                refs: vec![Ref {
                    symbol: "sc cargo dep 2.0 dep/helper().".to_string(),
                    line: 5,
                    col: 8,
                    role: "read",
                }],
            },
        );
        ingest.symbols.insert(
            "sc cargo p 1.0 store/Store#get().".to_string(),
            Info {
                kind: "method",
                name: "get".to_string(),
                doc: Some("Fetch one.".to_string()),
                sig: Some("fn get(&self) -> u32".to_string()),
                enclosing: None,
                implements: vec!["sc cargo p 1.0 store/Store#".to_string()],
            },
        );
        ingest.externs.insert(
            "sc cargo dep 2.0 dep/helper().".to_string(),
            ("cargo".to_string(), "dep".to_string(), "2.0".to_string()),
        );
        ingest
    }

    const SRC: &str = "pub struct Store {\n    n: u32,\n}\nimpl Store {\n    fn get(&self) -> \
                       u32 {\n        helper()\n    }\n}\n";

    #[test]
    fn an_anchor_carries_identity_owner_doc_and_signature() {
        let anchors = Anchors::of(&ingest(), "src/store.rs");
        assert_eq!(anchors.len(), 2);
        let get = anchors.at(4, 7).expect("the method is anchored");
        assert_eq!(get.symbol, "sc cargo p 1.0 store/Store#get().");
        // Rule 1: the descriptor prefix names a definition this index has.
        assert_eq!(get.parent.as_deref(), Some("sc cargo p 1.0 store/Store#"));
        assert_eq!(get.doc.as_deref(), Some("Fetch one."));
        assert_eq!(get.sig.as_deref(), Some("fn get(&self) -> u32"));
        // A position no definition starts at has no anchor, which is what
        // leaves tier A's synthesized symbol in place.
        assert!(anchors.at(4, 8).is_none());
        assert!(Anchors::of(&ingest(), "src/other.rs").is_empty());
    }

    #[test]
    fn a_descriptor_prefix_nothing_defines_does_not_become_a_parent() {
        let mut ingest = ingest();
        // Drop the type, keep the method: the truncated symbol now names
        // nothing, so rule 1 must not fire.
        ingest
            .docs
            .get_mut("src/store.rs")
            .expect("the document")
            .defs
            .remove(0);
        let anchors = Anchors::of(&ingest, "src/store.rs");
        assert_eq!(anchors.at(4, 7).expect("anchored").parent, None);
    }

    #[test]
    fn enclosing_symbol_is_the_fallback_when_the_prefix_is_not_indexed() {
        let mut ingest = ingest();
        ingest.docs.get_mut("src/store.rs").expect("doc").defs[1].symbol = "local a 4".to_string();
        ingest.symbols.insert(
            "local a 4".to_string(),
            Info {
                kind: "variable",
                name: "n".to_string(),
                enclosing: Some("sc cargo p 1.0 store/Store#".to_string()),
                ..Info::default()
            },
        );
        let anchors = Anchors::of(&ingest, "src/store.rs");
        assert_eq!(
            anchors.at(4, 7).expect("anchored").parent.as_deref(),
            Some("sc cargo p 1.0 store/Store#")
        );
    }

    fn emitted(anchored: &[(Span, String)]) -> (Segment, Counts, Interner) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut interner = Interner::open(dir.path()).expect("interner");
        let mut seg = Segment::new();
        let counts = emit(
            &mut seg,
            &ingest(),
            "src/store.rs",
            Some(SRC),
            anchored,
            &mut interner,
        )
        .expect("emits");
        (seg, counts, interner)
    }

    fn rows(seg: &Segment, relation: &str, interner: &Interner) -> Vec<String> {
        let mut out = Vec::new();
        for (rel, rows) in seg.iter() {
            if rel.name != relation {
                continue;
            }
            for row in rows.iter() {
                out.push(
                    row.iter()
                        .map(|a| {
                            interner
                                .resolve(*a)
                                .map_or_else(|| a.to_string(), str::to_string)
                        })
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
        }
        out.sort();
        out
    }

    #[test]
    fn a_definition_tier_a_already_owns_gets_only_a_resolved_row() {
        let anchored = vec![
            ((0, 31), "sc cargo p 1.0 store/Store#".to_string()),
            ((36, 76), "sc cargo p 1.0 store/Store#get().".to_string()),
        ];
        let (seg, counts, interner) = emitted(&anchored);
        assert_eq!(counts.resolved, 2);
        assert_eq!(counts.only, 0, "tier A owns both, so tier B adds no def");
        assert!(rows(&seg, "def", &interner).is_empty());
        assert_eq!(rows(&seg, "resolved", &interner).len(), 2);
    }

    #[test]
    fn a_definition_tier_a_missed_becomes_a_tier_b_def_with_no_span() {
        let (seg, counts, interner) = emitted(&[]);
        assert_eq!(counts.only, 2);
        assert_eq!(
            rows(&seg, "def", &interner),
            vec![
                "sc cargo p 1.0 store/Store# src/store.rs unknown ",
                "sc cargo p 1.0 store/Store#get(). src/store.rs method get",
            ]
        );
        // No span and no name position: SCIP's range is not our `def_span`,
        // and an approximate one puts an edit in the wrong place.
        assert!(rows(&seg, "def_span", &interner).is_empty());
        assert!(rows(&seg, "def_name", &interner).is_empty());
        assert_eq!(
            rows(&seg, "parent", &interner),
            vec![
                "sc cargo p 1.0 store/Store# src/store.rs",
                "sc cargo p 1.0 store/Store#get(). sc cargo p 1.0 store/Store#",
            ]
        );
    }

    #[test]
    fn a_reference_is_attributed_to_the_definition_it_sits_inside() {
        let anchored = vec![
            ((0, 31), "sc cargo p 1.0 store/Store#".to_string()),
            ((36, 76), "sc cargo p 1.0 store/Store#get().".to_string()),
        ];
        let (seg, counts, interner) = emitted(&anchored);
        assert_eq!(counts.refs, 1);
        assert_eq!(
            rows(&seg, "scip_ref", &interner),
            vec![
                "sc cargo dep 2.0 dep/helper(). src/store.rs 5 8 sc cargo p 1.0 store/Store#get(). \
                 read"
            ]
        );
    }

    #[test]
    fn a_reference_inside_nothing_is_attributed_to_its_file() {
        let (seg, _, interner) = emitted(&[]);
        // With no tier-A spans, only SCIP's enclosing ranges are available —
        // and `get`'s covers the reference, so it still lands inside it.
        assert_eq!(
            rows(&seg, "scip_ref", &interner),
            vec![
                "sc cargo dep 2.0 dep/helper(). src/store.rs 5 8 sc cargo p 1.0 store/Store#get(). \
                 read"
            ]
        );
    }

    #[test]
    fn an_implementation_and_a_package_are_recorded_once_each() {
        let anchored = vec![((36, 76), "sc cargo p 1.0 store/Store#get().".to_string())];
        let (seg, _, interner) = emitted(&anchored);
        assert_eq!(
            // No provenance column since schema 2: everything the indexer said
            // is `exact` by construction, and `implements(S, T, "exact")` is
            // the rule over this in `stdlib.dl`.
            rows(&seg, "scip_impl", &interner),
            vec!["sc cargo p 1.0 store/Store#get(). sc cargo p 1.0 store/Store#"]
        );
        assert_eq!(
            rows(&seg, "extern", &interner),
            vec!["sc cargo dep 2.0 dep/helper(). cargo dep 2.0"]
        );
    }

    #[test]
    fn a_file_scip_never_saw_produces_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut interner = Interner::open(dir.path()).expect("interner");
        let mut seg = Segment::new();
        let counts = emit(
            &mut seg,
            &ingest(),
            "src/other.rs",
            Some(""),
            &[],
            &mut interner,
        )
        .expect("emits");
        assert_eq!(counts, Counts::default());
        assert_eq!(seg.iter().count(), 0);
    }

    #[test]
    fn a_position_past_the_end_clamps_instead_of_wrapping() {
        let lines = LineIndex::of("ab\ncd\n");
        assert_eq!(lines.byte(1, 0), 0);
        assert_eq!(lines.byte(2, 1), 4);
        // Beyond the file: the source moved under the index. Clamp, never wrap
        // to zero and claim the first definition.
        assert_eq!(lines.byte(99, 0), 6);
        assert_eq!(lines.byte(2, 99), 6);
        assert_eq!(lines.byte(0, 0), 0);
    }
}
