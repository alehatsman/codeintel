//! Tier A: the tree-sitter tier.
//!
//! One parse per file, our authored queries over it, one span sweep, and rows
//! out. **Every fact is a function of its own file and nothing else**
//! (`specs/00-overview.md` invariant 3b): no name is resolved, no other file
//! is consulted, and what a name denotes is decided by `rules/stdlib.dl` at
//! query time. Freezing resolution into a per-file fact is what makes
//! incremental indexing unsound, so the extractor is not allowed to be clever.

use facts::{Interner, Segment};
use tree_sitter::{Node, Parser, Query, QueryCursor, StreamingIterator};

use crate::error::{Error, Result};
use crate::lang::{Export, Lang, TYPE_LIKE};
use crate::sweep::{self, Span};
use crate::symbol;

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
    exported: bool,
    sig: String,
    doc: Option<String>,
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
    /// # Errors
    /// A file that does not parse, a query capture that is not a `Kind`, a
    /// file past the integer-atom limit, or an exhausted dictionary.
    pub fn file(
        &mut self,
        path: &str,
        src: &str,
        interner: &mut Interner,
    ) -> Result<(Segment, Counts)> {
        let tree = self.parser.parse(src, None).ok_or_else(|| Error::Parse {
            path: path.to_string(),
        })?;
        let root = tree.root_node();

        let (items, occurrences) = self.scan(src, root)?;
        let mut spans: Vec<Span> = items.iter().map(|i| i.node).collect();
        spans.extend(occurrences.iter().map(|o| (o.at, o.at)));
        let parents = sweep::containment(&spans);
        let items = promote(items, &parents);
        let symbols = symbols_of(path, &items, &parents);

        let mut seg = Segment::new();
        let mut counts = Counts::default();
        let file = atom(interner, path)?;
        push(&mut seg, "file", &[file, atom(interner, self.lang.name)?]);

        for (i, item) in items.iter().enumerate() {
            if !item.is_def {
                continue;
            }
            let Some(Some(sym)) = symbols.get(i) else {
                continue;
            };
            let s = atom(interner, sym)?;
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
            if !item.sig.is_empty() {
                push(&mut seg, "def_sig", &[s, atom(interner, &item.sig)?]);
            }
            if let Some(doc) = &item.doc {
                push(&mut seg, "def_doc", &[s, atom(interner, doc)?]);
            }
            if item.exported {
                push(&mut seg, "exported", &[s]);
            }
            // Exactly one `parent` row per `def`, guaranteed by the sweep:
            // the innermost enclosing definition or scope, else the file.
            let owner = match parents
                .get(i)
                .copied()
                .flatten()
                .and_then(|j| symbols.get(j))
            {
                Some(Some(sym)) => atom(interner, sym)?,
                _ => file,
            };
            push(&mut seg, "parent", &[s, owner]);
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
        Ok((seg, counts))
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
            for capture in m.captures() {
                let Some(label) = self.tags.capture_names().get(capture.index as usize) else {
                    continue;
                };
                if *label == "name" {
                    name = Some(capture.node);
                } else {
                    subject = Some((*label, capture.node));
                }
            }
            let (Some((label, node)), Some(name)) = (subject, name) else {
                continue;
            };
            let text = name
                .utf8_text(src.as_bytes())
                .unwrap_or_default()
                .to_string();
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
            items.push(self.item(src, node, name, kind, is_def, text));
        }
        Ok((items, occurrences))
    }

    fn item(
        &self,
        src: &str,
        node: Node<'_>,
        name: Node<'_>,
        kind: &'static str,
        is_def: bool,
        text: String,
    ) -> Item {
        let (start, start_line, doc) = self.preamble(src, node);
        Item {
            kind,
            is_def,
            name: text,
            node: (node.start_byte(), node.end_byte()),
            span: (start, node.end_byte()),
            lines: (start_line, line_of_end(node)),
            name_at: (line_of(name), col_of(name)),
            exported: self.is_exported(src, node, name),
            sig: signature(src, node, self.lang.sig_stops),
            doc,
        }
    }

    /// Walk backwards over attributes and doc comments that belong to `node`.
    ///
    /// `def_span` is the **full** definition including attributes and the doc
    /// comment (`specs/01-facts.md` § `def_span`), and tree-sitter puts both in
    /// preceding siblings rather than inside the item. A blank line ends the
    /// run: two items separated by one are not each other's documentation.
    fn preamble(&self, src: &str, node: Node<'_>) -> (usize, u32, Option<String>) {
        let mut start = node.start_byte();
        let mut line = line_of(node);
        let mut docs: Vec<String> = Vec::new();
        let mut cur = node;
        while let Some(prev) = cur.prev_sibling() {
            let kind = prev.kind();
            let is_attr = self.lang.attribute_kinds.contains(&kind);
            let comment = self.lang.comment_kinds.contains(&kind);
            let text = prev.utf8_text(src.as_bytes()).unwrap_or_default();
            let is_doc = comment && self.lang.doc_markers.iter().any(|m| text.starts_with(m));
            if !is_attr && !is_doc {
                break;
            }
            let gap = src.get(prev.end_byte()..start).unwrap_or("");
            if !gap.chars().all(char::is_whitespace) || gap.matches('\n').count() > 1 {
                break;
            }
            if is_doc {
                docs.push(strip_markers(text, self.lang.doc_markers));
            }
            start = prev.start_byte();
            line = line_of(prev);
            cur = prev;
        }
        docs.reverse();
        let doc = docs.join("\n");
        (start, line, (!doc.trim().is_empty()).then_some(doc))
    }

    fn is_exported(&self, src: &str, node: Node<'_>, name: Node<'_>) -> bool {
        match self.lang.export {
            Export::ChildKind(kind) => {
                let mut cursor = node.walk();
                node.children(&mut cursor).any(|c| c.kind() == kind)
            }
            Export::Capitalized => name
                .utf8_text(src.as_bytes())
                .unwrap_or_default()
                .chars()
                .next()
                .is_some_and(char::is_uppercase),
            Export::NotUnderscored => !name
                .utf8_text(src.as_bytes())
                .unwrap_or_default()
                .starts_with('_'),
        }
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
fn symbols_of(path: &str, items: &[Item], parents: &[Option<usize>]) -> Vec<Option<String>> {
    (0..items.len())
        .map(|i| {
            let mut chain = Vec::new();
            let mut at = Some(i);
            while let Some(j) = at {
                let item = items.get(j)?;
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
/// It starts at the *syntactic* node, not at `def_span`'s start — the span
/// deliberately reaches back over attributes and the doc comment, and a
/// signature that opened with three lines of prose would be useless.
fn signature(src: &str, node: Node<'_>, stops: &[char]) -> String {
    let text = src.get(node.start_byte()..node.end_byte()).unwrap_or("");
    let end = text
        .char_indices()
        .find(|(_, c)| stops.contains(c) || *c == '\n')
        .map_or(text.len(), |(i, _)| i);
    collapse(text.get(..end).unwrap_or(text), SIG_CAP)
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
