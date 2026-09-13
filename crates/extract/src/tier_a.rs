//! Tier A: the tree-sitter tier.
//!
//! One parse per file, our authored queries over it, one span sweep, and rows
//! out. **Every fact is a function of its own file and nothing else**
//! (`specs/00-overview.md` invariant 3b): no name is resolved, no other file
//! is consulted, and what a name denotes is decided by `rules/stdlib.dl` at
//! query time. Freezing resolution into a per-file fact is what makes
//! incremental indexing unsound, so the extractor is not allowed to be clever.

use std::collections::{BTreeMap, BTreeSet};

use facts::{Interner, Segment};
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

use crate::error::{Error, Result};
use crate::lang::{Export, Lang, TYPE_LIKE, Vis};
use crate::sweep::{self, Span};
use crate::symbol;
use crate::tier_b::Anchors;

/// Longest `def_sig` we keep. It is a display string, not a parse.
const SIG_CAP: usize = 512;

/// How many facts one file produced, by shape. For `index`'s summary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// `def` rows.
    pub defs: usize,
    /// `name_ref` rows.
    pub refs: usize,
    /// `import` rows.
    pub imports: usize,
}

/// One file's tier-A output.
#[derive(Debug)]
pub struct Extracted {
    /// The facts.
    pub segment: Segment,
    /// How many, by shape.
    pub counts: Counts,
    /// Every definition's byte span and its **final** symbol — the SCIP one
    /// where the anchor join found a match. Tier B needs both: the spans to
    /// attribute its references, and the symbols to know which definitions it
    /// must not emit a second `def` row for.
    pub defs: Vec<(Span, String)>,
    /// Definitions that adopted a SCIP identity.
    pub anchored: usize,
}

/// A parser and compiled queries for one language, reused across files.
pub struct Extractor {
    lang: &'static Lang,
    parser: Parser,
    tags: Query,
    imports: Query,
}

/// One definition or scope found by `tags.scm`.
#[derive(Debug)]
struct Item {
    /// Its `Kind`. A scope carries one too — an `impl` block is a `type` — so
    /// that descriptor synthesis and method promotion need no special case.
    kind: &'static str,
    /// False for `@scope.*`: it owns things but defines nothing.
    is_def: bool,
    name: String,
    /// The syntactic node's byte range, which is where `def_sig` starts.
    node: Span,
    /// The node's range extended over attributes and doc comments, which is
    /// what `def_span` reports.
    span: Span,
    /// 1-based start and end lines of `span`.
    lines: (u32, u32),
    /// 1-based line and 0-based byte column of the identifier token.
    name_at: (u32, u32),
    /// The text of an `@owner` capture: an identifier *naming* this
    /// definition's owner, where the owner does not enclose it. `None` for
    /// every language that does not capture one.
    owner: Option<String>,
    /// What this definition states about its own visibility. Whether that makes
    /// it visible outside the crate is `stdlib.dl`'s question, not this one's.
    vis: Vis,
    /// True for a `@scope.impl` item: a block implementing a trait for a type.
    /// Definitions inside one cannot state a visibility, so they inherit.
    is_impl: bool,
    /// The trait a `@scope.impl` names, as the grammar's final identifier.
    /// `None` for everything else, an inherent `impl` included.
    trait_name: Option<String>,
    /// For a `@scope.impl` item, what qualifies it beyond its name: the trait's
    /// final identifier, plus the `@target` text when that is more than the
    /// bare name. Rendered as a `[...]` descriptor between the scope and its
    /// members, so two trait impls for one type give their members two symbols.
    qualifier: Option<String>,
    sig: String,
    doc: Option<String>,
}

/// What the preceding siblings of a definition node contributed to it.
#[derive(Debug)]
struct Preamble {
    /// Where `def_span` starts: back over attributes, keywords and the doc
    /// comment.
    start: usize,
    /// Where `def_sig` starts: back over adjacent keywords only. Attributes and
    /// the doc comment are part of the definition, not part of its header.
    sig_start: usize,
    /// 1-based line of `start`.
    start_line: u32,
    /// The doc comment, markers stripped, or `None` when there was none.
    doc: Option<String>,
}

/// One `tags.scm` match, before it becomes an [`Item`]. The captures the query
/// produced, gathered so that adding one is not another positional argument.
#[derive(Debug)]
struct Tag<'a> {
    /// The `@definition.*` or `@scope.*` node.
    node: Node<'a>,
    /// Its `@name` node.
    name: Node<'a>,
    kind: &'static str,
    is_def: bool,
    /// `name`'s source text.
    text: String,
    /// The `@owner` capture's source text, if the pattern had one.
    owner: Option<&'a str>,
    /// The `@doc` capture's source text: documentation the language keeps
    /// inside the definition (a Python docstring), where the preamble walk
    /// cannot see it.
    doc: Option<&'a str>,
    /// The `@trait` capture's source text: the trait a `@scope.impl` implements.
    trait_name: Option<&'a str>,
    /// The capture was `@scope.impl` rather than `@scope.type`.
    is_impl: bool,
    /// What qualifies a scope; see [`Item::qualifier`].
    qualifier: Option<String>,
}

/// One `@reference.call` occurrence.
#[derive(Debug)]
struct Occurrence {
    name: String,
    at: usize,
    line: u32,
    col: u32,
}

impl std::fmt::Debug for Extractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `tree_sitter::Parser` is not Debug, and `missing_debug_implementations`
        // is on for every public type. The compiled queries print as their whole
        // source, which is noise in a panic message, so only the language is
        // named.
        f.debug_struct("Extractor")
            .field("lang", &self.lang.name)
            .finish_non_exhaustive()
    }
}

impl Extractor {
    /// Compile the queries for a language and ready a parser.
    ///
    /// # Errors
    /// A grammar that will not load, or a `.scm` that does not compile.
    pub fn new(lang: &'static Lang) -> Result<Self> {
        let language = (lang.language)();
        let mut parser = Parser::new();
        parser.set_language(&language).map_err(|e| Error::Grammar {
            lang: lang.name,
            message: e.to_string(),
        })?;
        Ok(Self {
            lang,
            parser,
            tags: compile(lang, "tags", lang.tags, &language)?,
            imports: compile(lang, "imports", lang.imports, &language)?,
        })
    }

    /// Which language this extracts.
    #[must_use]
    pub const fn lang(&self) -> &'static Lang {
        self.lang
    }

    /// Extract one file into a segment.
    ///
    /// `path` is the repo-relative path with `/` separators — it is the file's
    /// identity in every relation, and it is baked into every symbol this file
    /// synthesizes.
    ///
    /// `anchors` is what SCIP knows about this file, empty when tier B has
    /// nothing for it. A definition whose name token SCIP also saw adopts the
    /// SCIP symbol **here**, before any fact is written, so every row that
    /// mentions it — `def_span`, `parent`, a reference's `From` — lands on the
    /// resolved identity for free and nothing is rewritten twice
    /// (`specs/02-extraction.md` § The anchor join).
    ///
    /// # Errors
    /// A file that does not parse, a query capture that is not a `Kind`, a
    /// file past the integer-atom limit, or an exhausted dictionary.
    pub fn file(
        &mut self,
        path: &str,
        src: &str,
        interner: &mut Interner,
        anchors: &Anchors,
    ) -> Result<Extracted> {
        let tree = self.parser.parse(src, None).ok_or_else(|| Error::Parse {
            path: path.to_string(),
        })?;
        let root = tree.root_node();

        let (items, occurrences) = self.scan(src, root)?;
        let mut spans: Vec<Span> = items.iter().map(|i| i.node).collect();
        spans.extend(occurrences.iter().map(|o| (o.at, o.at)));
        let mut parents = sweep::containment(&spans);
        reparent_by_owner(&items, &mut parents);
        let items = promote(items, &parents);
        let items = inherit_visibility(items, &parents, self.lang);
        let mut symbols = symbols_of(path, &items, &parents);
        let synthesized = symbols.clone();
        let mut anchored = 0;
        // What each anchored definition was called before it took its SCIP
        // identity. `symbols_of` gives a Rust `impl` block the same symbol as
        // the type it implements, and only the *type* carries a name token for
        // the join to land on — so anchoring rewrote the type and left the
        // `impl` block holding a name nothing defines any more. The block is
        // every trait-impl method's enclosing item, so the parent walk below
        // stepped over it to the file, and `exported` — which reaches a
        // method's visibility through its owner — stopped finding one.
        //
        // Measured on tests/fixtures/rust before this:
        //   tier A only   `?- def(S, "src/kinds.rs", _, "handle"), exported(S).` -> 3
        //   with SCIP     the same query                                         -> 1
        // Adding a *better* resolution tier removed rows, with `status: ok`.
        let mut renamed: BTreeMap<&str, Option<&str>> = BTreeMap::new();
        for (i, item) in items.iter().enumerate() {
            if let Some(anchor) = anchors.at(item.name_at.0, item.name_at.1)
                && item.is_def
                && let Some(slot) = symbols.get_mut(i)
                && slot.is_some()
            {
                *slot = Some(anchor.symbol.clone());
                anchored += 1;
            }
        }
        for (i, before) in synthesized.iter().enumerate() {
            let (Some(before), Some(Some(after))) = (before.as_deref(), symbols.get(i)) else {
                continue;
            };
            if before == after || !items.get(i).is_some_and(|it| it.is_def) {
                continue;
            }
            // Two definitions sharing one synthesized name and anchoring apart
            // is an ambiguity, not a rename. `None` poisons the entry so the
            // rewrite below leaves those items alone rather than picking one.
            renamed
                .entry(before)
                .and_modify(|slot| {
                    if *slot != Some(after.as_str()) {
                        *slot = None;
                    }
                })
                .or_insert(Some(after));
        }
        if !renamed.is_empty() {
            let stale: Vec<(usize, String)> = symbols
                .iter()
                .enumerate()
                .filter(|(i, _)| !items.get(*i).is_some_and(|it| it.is_def))
                .filter_map(|(i, sym)| {
                    let target = renamed.get(sym.as_deref()?)?.as_ref()?;
                    Some((i, (*target).to_string()))
                })
                .collect();
            for (i, target) in stale {
                if let Some(slot) = symbols.get_mut(i) {
                    *slot = Some(target);
                }
            }
        }

        let mut seg = Segment::new();
        let mut counts = Counts::default();
        let mut defs: Vec<(Span, String)> = Vec::new();
        let file = atom(interner, path)?;
        push(&mut seg, "file", &[file, atom(interner, self.lang.name)?]);

        // Every symbol this file is about to emit a `def` for, so precedence
        // rule 3 can apply the same existence check rules 1 and 2 apply
        // (`specs/02-extraction.md` § Parent precedence). The check is on the
        // *symbol*, not on the item: a Rust `impl` block is not a `def`, but
        // `symbols_of` gives it the same symbol as the type it implements, so
        // in a tier-A-only index it names a definition and is a fine parent.
        // Anchoring is what breaks that — the type takes its SCIP identity and
        // the `impl` block keeps the stale local one, which then names nothing.
        let defined: BTreeSet<&str> = items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.is_def)
            .filter_map(|(i, _)| symbols.get(i)?.as_deref())
            .collect();

        for (i, item) in items.iter().enumerate() {
            if !item.is_def {
                continue;
            }
            let Some(Some(sym)) = symbols.get(i) else {
                continue;
            };
            let s = atom(interner, sym)?;
            let anchor = anchors.at(item.name_at.0, item.name_at.1);
            defs.push((item.node, sym.clone()));
            counts.defs += 1;
            push(
                &mut seg,
                "def",
                &[
                    s,
                    file,
                    atom(interner, item.kind)?,
                    atom(interner, &item.name)?,
                ],
            );
            push(
                &mut seg,
                "def_span",
                &[
                    s,
                    int(path, item.lines.0)?,
                    int(path, item.lines.1)?,
                    offset(path, item.span.0)?,
                    offset(path, item.span.1)?,
                ],
            );
            push(
                &mut seg,
                "def_name",
                &[s, int(path, item.name_at.0)?, int(path, item.name_at.1)?],
            );
            // Tier B's text is a fallback, never an override: tier A read the
            // file, so where it found a signature or a doc comment that is the
            // one with a byte range behind it.
            if !item.sig.is_empty() {
                push(&mut seg, "def_sig", &[s, atom(interner, &item.sig)?]);
            } else if let Some(sig) = anchor.and_then(|a| a.sig.as_deref()) {
                push(&mut seg, "def_sig", &[s, atom(interner, sig)?]);
            }
            if let Some(doc) = item
                .doc
                .as_deref()
                .or(anchor.and_then(|a| a.doc.as_deref()))
            {
                push(&mut seg, "def_doc", &[s, atom(interner, doc)?]);
            }
            // Always a row, never a conditional one: `visibility` is a total
            // function of the definition, and an absent row would be a third
            // value the vocabulary does not have.
            push(
                &mut seg,
                "visibility",
                &[s, atom(interner, item.vis.as_str())?],
            );
            // Exactly one `parent` row per `def`. `parent` carries the
            // *semantic* owner, so tier B's answer replaces this one rather
            // than adding a second row — a Go method is lexically at file
            // scope and semantically owned by its type
            // (`specs/02-extraction.md` § Parent precedence). Where tier B has
            // no answer, the sweep's innermost enclosing span *that this file
            // actually defines* stands, else the file. Taking the innermost
            // enclosing span unchecked named a symbol with no `def` row, which
            // stopped `within/2` one hop short of the file without failing the
            // query — a short answer no status code can report.
            let owner = match anchor.and_then(|a| a.parent.as_deref()) {
                Some(parent) => atom(interner, parent)?,
                None => match ancestor(&parents, i, |j| {
                    symbols
                        .get(j)
                        .and_then(Option::as_deref)
                        .is_some_and(|sym| defined.contains(sym))
                })
                .and_then(|j| symbols.get(j))
                {
                    Some(Some(sym)) => atom(interner, sym)?,
                    _ => file,
                },
            };
            push(&mut seg, "parent", &[s, owner]);
        }

        // `impl Trait for Type` -> `name_impl`. Both names as written, neither
        // resolved: turning them into symbols needs the whole repo, so it is
        // the rule layer's job (`specs/01-facts.md` § `name_impl`). An inherent
        // `impl` carries no `@trait` capture and so emits nothing.
        for item in items.iter().filter(|i| i.is_impl) {
            let Some(trait_name) = item.trait_name.as_deref() else {
                continue;
            };
            push(
                &mut seg,
                "name_impl",
                &[
                    file,
                    atom(interner, &item.name)?,
                    atom(interner, trait_name)?,
                    int(path, item.lines.0)?,
                ],
            );
        }

        for (k, occurrence) in occurrences.iter().enumerate() {
            let from = ancestor(&parents, items.len() + k, |j| {
                items.get(j).is_some_and(|i| i.is_def)
            })
            .and_then(|j| symbols.get(j).cloned().flatten());
            let from = match &from {
                Some(sym) => atom(interner, sym)?,
                None => file,
            };
            counts.refs += 1;
            push(
                &mut seg,
                "name_ref",
                &[
                    atom(interner, &occurrence.name)?,
                    file,
                    int(path, occurrence.line)?,
                    int(path, occurrence.col)?,
                    from,
                ],
            );
        }

        counts.imports = self.emit_imports(src, root, file, interner, &mut seg)?;
        Ok(Extracted {
            segment: seg,
            counts,
            defs,
            anchored,
        })
    }

    /// Run `tags.scm` and turn its matches into items and occurrences.
    fn scan(&self, src: &str, root: Node<'_>) -> Result<(Vec<Item>, Vec<Occurrence>)> {
        let mut items = Vec::new();
        let mut occurrences = Vec::new();
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.tags, root, src.as_bytes());
        while let Some(m) = matches.next() {
            let mut subject = None;
            let mut name = None;
            let mut owner = None;
            let mut doc = None;
            let mut trait_name = None;
            let mut target = None;
            let mut value = None;
            for capture in m.captures() {
                let Some(label) = self.tags.capture_names().get(capture.index as usize) else {
                    continue;
                };
                match *label {
                    "name" => name = Some(capture.node),
                    "owner" => owner = capture.node.utf8_text(src.as_bytes()).ok(),
                    "doc" => doc = capture.node.utf8_text(src.as_bytes()).ok(),
                    "trait" => trait_name = capture.node.utf8_text(src.as_bytes()).ok(),
                    "target" => target = capture.node.utf8_text(src.as_bytes()).ok(),
                    "value" => value = Some(capture.node.kind()),
                    label => subject = Some((label, capture.node)),
                }
            }
            let (Some((label, node)), Some(name)) = (subject, name) else {
                continue;
            };
            let text = name
                .utf8_text(src.as_bytes())
                .unwrap_or_default()
                .to_string();
            // As written, whitespace collapsed. The target joins only when it
            // says more than the name does: `impl T for A` and `impl T for &A`
            // are two impls and must not share a symbol.
            let qualifier = trait_name.map(|t| match target {
                Some(target) if target != text => {
                    format!("{t} for {}", collapse(target, usize::MAX))
                }
                _ => t.to_string(),
            });
            if label == "reference.call" {
                occurrences.push(Occurrence {
                    name: text,
                    at: name.start_byte(),
                    line: line_of(name),
                    col: col_of(name),
                });
                continue;
            }
            let (is_def, suffix) = match label.split_once('.') {
                Some(("definition", suffix)) => (true, suffix),
                Some(("scope", suffix)) => (false, suffix),
                _ => continue,
            };
            let kind = self.lang.kind(suffix).ok_or_else(|| Error::Capture {
                lang: self.lang.name,
                capture: (*label).to_string(),
            })?;
            // A binding whose value node is a function is a `function`,
            // whatever its keyword said: the grammar states it, and `calls`
            // counts only callable kinds (`specs/02-extraction.md` § `@value`).
            let kind = if value.is_some_and(|v| self.lang.function_values.contains(&v)) {
                "function"
            } else {
                kind
            };
            items.push(self.item(
                src,
                Tag {
                    node,
                    name,
                    kind,
                    is_def,
                    text,
                    owner,
                    doc,
                    trait_name,
                    is_impl: !is_def && suffix == "impl",
                    qualifier,
                },
            ));
        }
        Ok((items, occurrences))
    }

    fn item(&self, src: &str, tag: Tag<'_>) -> Item {
        let Preamble {
            start,
            sig_start,
            start_line,
            doc,
        } = self.preamble(src, tag.node);
        Item {
            kind: tag.kind,
            is_def: tag.is_def,
            name: tag.text,
            node: (tag.node.start_byte(), tag.node.end_byte()),
            span: (start, tag.node.end_byte()),
            lines: (start_line, line_of_end(tag.node)),
            name_at: (line_of(tag.name), col_of(tag.name)),
            owner: tag.owner.map(str::to_string),
            vis: self.visibility(src, tag.node, tag.name),
            is_impl: tag.is_impl,
            trait_name: tag.trait_name.map(str::to_string),
            qualifier: tag.qualifier,
            sig: signature(src, sig_start, tag.node.end_byte(), self.lang.sig_stops),
            // A `@doc` capture is the documentation where the language keeps
            // it inside the definition; the preamble is where it keeps it in
            // front. A language states one or the other in its query and its
            // `doc_markers`, so the two do not compete. The same per-line
            // strip a comment gets, with no marker: a docstring's lines are
            // indented to the body.
            doc: tag
                .doc
                .map(|d| strip_markers(d, &[]))
                .filter(|d| !d.is_empty())
                .or(doc),
        }
    }

    /// Walk backwards over attributes and doc comments that belong to `node`.
    ///
    /// `def_span` is the **full** definition including attributes and the doc
    /// comment (`specs/01-facts.md` § `def_span`), and tree-sitter puts both in
    /// preceding siblings rather than inside the item. A blank line ends the
    /// run: two items separated by one are not each other's documentation.
    fn preamble(&self, src: &str, node: Node<'_>) -> Preamble {
        let mut start = node.start_byte();
        let mut sig_start = node.start_byte();
        // Only a run of keywords *adjacent* to the node is part of the
        // signature. Once anything else has been walked over, a keyword further
        // back belongs to something else.
        let mut adjacent = true;
        let mut line = line_of(node);
        let mut docs: Vec<String> = Vec::new();
        let mut cur = node;
        loop {
            let Some(prev) = cur.prev_sibling() else {
                // Nothing else precedes us inside `cur`'s parent, so whatever
                // precedes the parent precedes us too — but only if the parent
                // begins where we have already walked back to. Go needs this:
                // the definition is the `type_spec`, the `type` keyword is its
                // only preceding sibling, and the doc comment is a sibling of
                // the `type_declaration` one level up. Each step strictly grows
                // the node, so this terminates at the root.
                match cur.parent().filter(|p| p.start_byte() == start) {
                    Some(parent) => {
                        cur = parent;
                        continue;
                    }
                    None => break,
                }
            };
            let kind = prev.kind();
            let is_attr = self.lang.attribute_kinds.contains(&kind);
            let is_keyword = self.lang.keyword_kinds.contains(&kind);
            let comment = self.lang.comment_kinds.contains(&kind);
            let text = prev.utf8_text(src.as_bytes()).unwrap_or_default();
            let is_doc = comment && self.lang.doc_markers.iter().any(|m| text.starts_with(m));
            if !is_attr && !is_keyword && !is_doc {
                break;
            }
            let gap = src.get(prev.end_byte()..start).unwrap_or("");
            if !gap.chars().all(char::is_whitespace) || gap.matches('\n').count() > 1 {
                break;
            }
            if is_doc {
                docs.push(strip_markers(text, self.lang.doc_markers));
            }
            if is_keyword && adjacent {
                sig_start = prev.start_byte();
            } else {
                adjacent = false;
            }
            start = prev.start_byte();
            line = line_of(prev);
            cur = prev;
        }
        docs.reverse();
        let doc = docs.join("\n");
        Preamble {
            start,
            sig_start,
            start_line: line,
            doc: (!doc.trim().is_empty()).then_some(doc),
        }
    }

    /// What this node *states* about its visibility — never what that implies.
    ///
    /// `specs/01-facts.md` § `visibility(S, Vis)`. The ancestor walk that turns
    /// `inherited` into an answer lives in `stdlib.dl`, so that a variant of a
    /// public enum inside a private module comes out right without this
    /// function knowing what a module is.
    fn visibility(&self, src: &str, node: Node<'_>, name: Node<'_>) -> Vis {
        let public = match self.lang.export {
            Export::ChildWord { kind, words } => {
                let mut cursor = node.walk();
                node.children(&mut cursor)
                    .filter(|c| c.kind() == kind)
                    .any(|c| words.contains(&c.utf8_text(src.as_bytes()).unwrap_or_default()))
            }
            Export::Capitalized => name
                .utf8_text(src.as_bytes())
                .unwrap_or_default()
                .chars()
                .next()
                .is_some_and(char::is_uppercase),
            Export::NotUnderscored => {
                let text = name.utf8_text(src.as_bytes()).unwrap_or_default();
                !text.starts_with('_') || (text.starts_with("__") && text.ends_with("__"))
            }
            Export::Statement {
                wrapper,
                through,
                members,
                modifier,
                restricted,
                private_name,
            } => {
                if members.contains(&node.kind()) {
                    let mut cursor = node.walk();
                    let restricts = node
                        .children(&mut cursor)
                        .filter(|c| c.kind() == modifier)
                        .any(|c| {
                            restricted.contains(&c.utf8_text(src.as_bytes()).unwrap_or_default())
                        });
                    !restricts && name.kind() != private_name
                } else {
                    let mut up = node.parent();
                    while let Some(parent) = up.filter(|p| through.contains(&p.kind())) {
                        up = parent.parent();
                    }
                    up.is_some_and(|p| p.kind() == wrapper)
                }
            }
        };
        if public { Vis::Public } else { Vis::Restricted }
    }

    fn emit_imports(
        &self,
        src: &str,
        root: Node<'_>,
        file: u32,
        interner: &mut Interner,
        seg: &mut Segment,
    ) -> Result<usize> {
        let mut count = 0;
        let mut cursor = QueryCursor::new();
        let mut matches = cursor.matches(&self.imports, root, src.as_bytes());
        while let Some(m) = matches.next() {
            let mut module = None;
            let mut alias = "";
            for capture in m.captures() {
                match self.imports.capture_names().get(capture.index as usize) {
                    Some(&"module") => {
                        module = capture.node.utf8_text(src.as_bytes()).ok();
                    }
                    Some(&"alias") => {
                        alias = capture.node.utf8_text(src.as_bytes()).unwrap_or_default();
                    }
                    // An `export { x }` clause names a local; the declaration
                    // it exports is joined in `stdlib.dl`, not here
                    // (`specs/01-facts.md` § `name_export`). Not an import,
                    // so not in `count`.
                    Some(&"export") => {
                        let name = capture.node.utf8_text(src.as_bytes()).unwrap_or_default();
                        push(seg, "name_export", &[file, atom(interner, name)?]);
                    }
                    _ => {}
                }
            }
            let Some(module) = module else { continue };
            // As written, never resolved to a path: resolution is the SCIP
            // tier's job (specs/01-facts.md § `import`).
            let module = collapse(module, usize::MAX);
            push(
                seg,
                "import",
                &[file, atom(interner, &module)?, atom(interner, alias)?],
            );
            count += 1;
        }
        Ok(count)
    }
}

fn compile(
    lang: &'static Lang,
    which: &'static str,
    source: &str,
    language: &tree_sitter::Language,
) -> Result<Query> {
    Query::new(language, source).map_err(|e| Error::Query {
        lang: lang.name,
        which,
        message: e.to_string(),
    })
}

/// Apply every `@owner` capture that resolves, overriding the span sweep.
///
/// `specs/02-extraction.md` § `@owner`: a Go method is declared at file scope
/// and names its owner in its own receiver, so span nesting is not merely
/// imprecise there — it synthesizes `local store.go Get().`, which every type
/// in the file with a `Get` method would claim.
///
/// Candidates are the type-like definitions of this same file that **no other
/// definition encloses**. Restricting them to the top level is what keeps the
/// parent graph a forest: the only edge added runs from a nested definition to
/// an unnested one, so a `type Store struct{}` declared inside a method body
/// can never become that method's owner and close a cycle. Zero candidates or
/// two is no override, never a choice between them — an extractor that picked
/// would be guessing, which invariant 1 forbids.
fn reparent_by_owner(items: &[Item], parents: &mut [Option<usize>]) {
    if items.iter().all(|i| i.owner.is_none()) {
        return;
    }
    let mut candidates: BTreeMap<&str, Option<usize>> = BTreeMap::new();
    for (i, item) in items.iter().enumerate() {
        if !item.is_def
            || !TYPE_LIKE.contains(&item.kind)
            || parents.get(i).copied().flatten().is_some()
        {
            continue;
        }
        // `None` is the poison value for "named twice, so unusable", which is
        // why this is not a plain insert.
        candidates
            .entry(&item.name)
            .and_modify(|slot| *slot = None)
            .or_insert(Some(i));
    }
    for (i, item) in items.iter().enumerate() {
        let Some(owner) = item.owner.as_deref() else {
            continue;
        };
        if let Some(&Some(j)) = candidates.get(owner)
            && j != i
            && let Some(slot) = parents.get_mut(i)
        {
            *slot = Some(j);
        }
    }
}

/// A definition whose owner forbids it a visibility of its own `inherit`s one.
///
/// Rust rejects `pub` on an enum variant, on any trait item, and on a
/// trait-impl method, so a missing modifier in those places is silence rather
/// than privacy. The member's own node kind cannot detect it — a provided trait
/// method and a free function are both `function_item` — so the test is on the
/// owner: its `Kind` for the first two ([`Lang::vis_inherits_under`]), and the
/// `@scope.impl` capture for the third, which `tags.scm` separates from
/// `@scope.type` precisely because an inherent `impl` carries the same `Kind`
/// and its methods *may* say `pub`.
///
/// Done here for the same reason [`promote`] is: a query cannot see its own
/// nesting without matching the node twice.
fn inherit_visibility(mut items: Vec<Item>, parents: &[Option<usize>], lang: &Lang) -> Vec<Item> {
    let inherits: Vec<bool> = (0..items.len())
        .map(|i| {
            parents
                .get(i)
                .copied()
                .flatten()
                .and_then(|j| items.get(j))
                .is_some_and(|p| p.is_impl || lang.vis_inherits_under.contains(&p.kind))
        })
        .collect();
    for (item, inherits) in items.iter_mut().zip(inherits) {
        if inherits {
            item.vis = Vis::Inherited;
        }
    }
    items
}

/// A `function` owned by a type-like definition is a `method`.
///
/// Done here rather than in `tags.scm` because a query cannot see its own
/// nesting without matching the node twice — which is precisely how upstream
/// emits two `def` rows for every Rust method.
fn promote(mut items: Vec<Item>, parents: &[Option<usize>]) -> Vec<Item> {
    let owner_kind: Vec<Option<&'static str>> = (0..items.len())
        .map(|i| {
            parents
                .get(i)
                .copied()
                .flatten()
                .and_then(|j| items.get(j))
                .map(|p| p.kind)
        })
        .collect();
    for (item, owner) in items.iter_mut().zip(owner_kind) {
        if item.kind == "function" && owner.is_some_and(|k| TYPE_LIKE.contains(&k)) {
            item.kind = "method";
        }
    }
    items
}

/// The symbol for every item, or `None` for one whose kind has no descriptor.
///
/// A qualified scope — a trait impl — contributes its qualifier to its
/// *members'* chains and not to its own: the scope's own symbol stays the
/// type's, so `parent` still names the type, while `impl Display for A` and
/// `impl Debug for A` give their two `fmt` methods `A#[Display]fmt().` and
/// `A#[Debug]fmt().` rather than one symbol with two spans.
fn symbols_of(path: &str, items: &[Item], parents: &[Option<usize>]) -> Vec<Option<String>> {
    (0..items.len())
        .map(|i| {
            let mut chain = Vec::new();
            let mut at = Some(i);
            while let Some(j) = at {
                let item = items.get(j)?;
                if j != i
                    && let Some(qualifier) = &item.qualifier
                {
                    chain.push(format!("[{}]", symbol::escape(qualifier)));
                }
                chain.push(symbol::descriptor(item.kind, &item.name)?);
                at = parents.get(j).copied().flatten();
            }
            chain.reverse();
            Some(symbol::symbol(path, &chain))
        })
        .collect()
}

/// The nearest ancestor of `i` satisfying `want`.
fn ancestor(parents: &[Option<usize>], i: usize, want: impl Fn(usize) -> bool) -> Option<usize> {
    let mut at = parents.get(i).copied().flatten();
    while let Some(j) = at {
        if want(j) {
            return Some(j);
        }
        at = parents.get(j).copied().flatten();
    }
    None
}

/// The declaration header: source from the node's start to the first body
/// delimiter, whitespace-collapsed and capped.
///
/// It starts at the declaration head, not at `def_span`'s start — the span
/// deliberately reaches back over attributes and the doc comment, and a
/// signature that opened with three lines of prose would be useless.
fn signature(src: &str, start: usize, end: usize, stops: &[char]) -> String {
    let text = src.get(start..end).unwrap_or("");
    // A stop character nested inside a bracket group is part of the header, not
    // the end of it: `struct S<T = u32>` cut at the `=` used to report
    // `struct S<T`. `->` cannot underflow the depth because the subtraction
    // saturates.
    let mut depth = 0_u32;
    let mut cut = text.len();
    for (i, c) in text.char_indices() {
        match c {
            '<' | '(' | '[' => depth = depth.saturating_add(1),
            '>' | ')' | ']' => depth = depth.saturating_sub(1),
            '\n' => {
                cut = i;
                break;
            }
            _ if depth == 0 && stops.contains(&c) => {
                cut = i;
                break;
            }
            _ => {}
        }
    }
    collapse(text.get(..cut).unwrap_or(text), SIG_CAP)
}

/// Whitespace-collapsed, trimmed, and cut at a character boundary.
fn collapse(text: &str, cap: usize) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.len() <= cap {
        return joined;
    }
    let mut end = cap;
    while end > 0 && !joined.is_char_boundary(end) {
        end -= 1;
    }
    joined.get(..end).unwrap_or_default().to_string()
}

/// Comment text with its markers and one following space removed per line.
fn strip_markers(text: &str, markers: &[&str]) -> String {
    text.lines()
        .map(|line| {
            let line = line.trim_start();
            let line = markers
                .iter()
                .find_map(|m| line.strip_prefix(m))
                .unwrap_or(line);
            let line = line.trim_end().trim_end_matches("*/");
            line.strip_prefix(' ').unwrap_or(line).trim_end()
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn line_of(node: Node<'_>) -> u32 {
    u32::try_from(node.start_position().row.saturating_add(1)).unwrap_or(u32::MAX)
}

fn line_of_end(node: Node<'_>) -> u32 {
    u32::try_from(node.end_position().row.saturating_add(1)).unwrap_or(u32::MAX)
}

fn col_of(node: Node<'_>) -> u32 {
    u32::try_from(node.start_position().column).unwrap_or(u32::MAX)
}

fn push(seg: &mut Segment, relation: &str, row: &[u32]) {
    // A refused row is an arity bug against the compile-time relation table,
    // which no input can cause; ignoring it here would hide it in the answer.
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

fn offset(path: &str, n: usize) -> Result<u32> {
    i64::try_from(n)
        .ok()
        .and_then(datalog::atom::int_atom)
        .ok_or_else(|| Error::TooLarge {
            path: path.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::signature;

    /// The stop characters Rust declares (`lang.rs`), duplicated here so the
    /// test states what it exercises.
    const RUST: &[char] = &['{', ';', '='];

    #[test]
    fn a_stop_character_inside_brackets_does_not_end_the_header() {
        // Each of these used to be cut at the first `=` or `;`, reporting a
        // signature with an unclosed bracket in it.
        let cases = [
            ("struct S<T = u32>;", "struct S<T = u32>"),
            ("halves: [u16; 2]", "halves: [u16; 2]"),
            (
                "fn f<const N: usize = 4>(a: [u8; N]) {",
                "fn f<const N: usize = 4>(a: [u8; N])",
            ),
        ];
        for (src, want) in cases {
            assert_eq!(signature(src, 0, src.len(), RUST), want, "{src}");
        }
    }

    #[test]
    fn a_stop_character_at_the_top_level_still_ends_the_header() {
        assert_eq!(
            signature("pub const LIMIT: u32 = 64;", 0, 26, RUST),
            "pub const LIMIT: u32"
        );
        assert_eq!(
            signature("fn go() -> bool { true }", 0, 24, RUST),
            "fn go() -> bool"
        );
    }
}
