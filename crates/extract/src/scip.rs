//! Reading `index.scip` into something per-file.
//!
//! Everything here is normalization, not interpretation: positions become our
//! convention, SCIP's 80-odd kinds collapse onto our sixteen, document-scoped
//! `local N` symbols get their document back, and descriptors are *parsed*
//! rather than sliced. What the facts then say is [`crate::tier_b`]'s job.
//!
//! `specs/02-extraction.md` § Ingest is the contract, field for field.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use protobuf::Message as _;
use scip::types::{Document, Index, Occurrence, PositionEncoding, SymbolInformation, SymbolRole};

use crate::error::{Error, Result};

/// SCIP's kind vocabulary collapsed onto ours (`specs/02-extraction.md`).
///
/// A table, deliberately: every row is a name from `scip.proto` on the left and
/// a member of [`crate::lang::KINDS`] on the right, so a reader can check it
/// against the spec by eye. Anything absent is `unknown` — which is the honest
/// answer for a kind we have no vocabulary for, and is *not* a guess.
///
/// `Union` maps to `type`, not `enum`. Tier A's `kind_remap` agrees, so the
/// anchor join never has to reconcile a kind.
pub(crate) const KIND_MAP: &[(&str, &str)] = &[
    ("Function", "function"),
    ("Macro", "macro"),
    ("Method", "method"),
    ("AbstractMethod", "method"),
    ("StaticMethod", "method"),
    ("SingletonMethod", "method"),
    ("TraitMethod", "method"),
    ("ProtocolMethod", "method"),
    ("PureVirtualMethod", "method"),
    ("MethodSpecification", "method"),
    ("TypeClassMethod", "method"),
    ("Getter", "method"),
    ("Setter", "method"),
    ("Accessor", "method"),
    ("MethodAlias", "method"),
    ("Constructor", "constructor"),
    ("Class", "class"),
    ("SingletonClass", "class"),
    ("Struct", "struct"),
    ("Interface", "interface"),
    ("Protocol", "interface"),
    ("Trait", "trait"),
    ("TypeClass", "trait"),
    ("Concept", "trait"),
    ("Enum", "enum"),
    ("Union", "type"),
    ("Field", "field"),
    ("Property", "field"),
    ("StaticField", "field"),
    ("StaticProperty", "field"),
    ("StaticDataMember", "field"),
    ("Key", "field"),
    ("Constant", "constant"),
    ("EnumMember", "constant"),
    ("Variable", "variable"),
    ("StaticVariable", "variable"),
    ("Value", "variable"),
    ("Object", "variable"),
    ("Instance", "variable"),
    ("Module", "module"),
    ("Namespace", "module"),
    ("Package", "module"),
    ("PackageObject", "module"),
    ("File", "module"),
    ("Type", "type"),
    ("TypeAlias", "typealias"),
    ("TypeFamily", "typealias"),
    ("DataFamily", "typealias"),
    ("AssociatedType", "typealias"),
];

/// `symbol_roles` bits, most specific first.
///
/// One occurrence can carry several; we report the first that matches, in this
/// order, because `Role` is one atom and an occurrence that is both a write and
/// a test is more usefully a `write`.
const ROLES: &[(SymbolRole, &str)] = &[
    (SymbolRole::Definition, "def"),
    (SymbolRole::ForwardDefinition, "forward"),
    (SymbolRole::WriteAccess, "write"),
    (SymbolRole::ReadAccess, "read"),
    (SymbolRole::Import, "import"),
    (SymbolRole::Test, "test"),
    (SymbolRole::Generated, "generated"),
];

/// One SCIP definition occurrence, normalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Def {
    /// The symbol string, with `local N` already rewritten to carry its path.
    pub symbol: String,
    /// 1-based line of the identifier token.
    pub line: u32,
    /// 0-based UTF-8 byte column of the identifier token.
    pub col: u32,
    /// The full-definition range, when the indexer supplied one: byte-free,
    /// `(start_line, start_col, end_line, end_col)`. Tier B uses it to
    /// attribute references in a file tier A never parsed.
    pub enclosing: Option<(u32, u32, u32, u32)>,
}

/// One SCIP reference occurrence, normalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    /// The symbol string, with `local N` already rewritten.
    pub symbol: String,
    /// 1-based line.
    pub line: u32,
    /// 0-based UTF-8 byte column.
    pub col: u32,
    /// Its `Role`, from the `symbol_roles` bitset.
    pub role: &'static str,
}

/// One document's occurrences, after normalization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Doc {
    /// `Document.language`, lowercased.
    pub lang: String,
    /// Occurrences carrying the `Definition` role.
    pub defs: Vec<Def>,
    /// Every other occurrence.
    pub refs: Vec<Ref>,
}

/// What a `SymbolInformation` told us about one symbol.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    /// Our `Kind`, via [`kind_of`].
    pub kind: &'static str,
    /// `display_name`.
    pub name: String,
    /// `documentation`, joined.
    pub doc: Option<String>,
    /// `signature_documentation.text`.
    pub sig: Option<String>,
    /// `enclosing_symbol`, rewritten if it is a local.
    pub enclosing: Option<String>,
    /// Symbols this one implements, from `Relationship.is_implementation`.
    pub implements: Vec<String>,
}

/// A whole SCIP index, normalized and keyed by repo-relative path.
#[derive(Debug, Clone, Default)]
pub struct Ingest {
    /// `Metadata.tool_info`, as `name version`, for `status`.
    pub tool: String,
    /// Documents ingested, by `relative_path`.
    pub docs: BTreeMap<String, Doc>,
    /// Everything a `SymbolInformation` said, by symbol string.
    pub symbols: BTreeMap<String, Info>,
    /// `(manager, package, version)` for every symbol that names a package and
    /// is **not** defined by any document in this index.
    ///
    /// Not defined here is the whole test. `specs/02-extraction.md` says
    /// "package ≠ project package", and the set of packages a project is is
    /// not a thing SCIP states — but the set of symbols it defines is, exactly,
    /// so the complement needs no guess.
    pub externs: BTreeMap<String, (String, String, String)>,
    /// Symbols this index defines in more than one document.
    ///
    /// rust-analyzer keys symbols by package, not by cargo target, so
    /// `crate/` and `main().` are each defined once per binary, test and
    /// example target. One `SymId` with two definitions breaks every rule
    /// that assumes one `def` per symbol — `at/3` cross-multiplies,
    /// `innermost_at` answers from the wrong file — so the anchor join
    /// refuses them and tier A keeps its own identity
    /// (`specs/02-extraction.md` § The anchor join).
    pub collisions: BTreeSet<String>,
    /// Documents skipped, with the reason. Never silent: an approximate column
    /// breaks every edit built on it, so a document we cannot place exactly is
    /// dropped and reported (`specs/02-extraction.md` § Position normalization).
    pub skipped: Vec<(String, &'static str)>,
    /// Occurrences skipped because their document declares no position
    /// encoding and the line is not ASCII before the column
    /// (`specs/02-extraction.md` § Position normalization). Counted, because a
    /// skip nobody reports reads as a reference that was never there.
    pub ambiguous: usize,
}

impl Ingest {
    /// Read and normalize one `index.scip`.
    ///
    /// `root` is the repository root, used to find a document's source when it
    /// did not carry its own text and its positions need transcoding.
    ///
    /// # Errors
    /// The file cannot be read, or it is not a SCIP index.
    pub fn read(path: &Path, root: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| Error::Scip {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        let index = Index::parse_from_bytes(&bytes).map_err(|e| Error::Scip {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        Ok(Self::of(&index, root))
    }

    /// Normalize an already-parsed index.
    #[must_use]
    pub fn of(index: &Index, root: &Path) -> Self {
        let mut out = Self {
            tool: index
                .metadata
                .as_ref()
                .and_then(|m| m.tool_info.as_ref())
                .map_or_else(String::new, |t| {
                    format!("{} {}", t.name, t.version).trim().to_string()
                }),
            ..Self::default()
        };

        let mut candidates: BTreeMap<String, (String, String, String)> = BTreeMap::new();
        for document in &index.documents {
            out.document(document, root, &mut candidates);
        }
        for info in &index.external_symbols {
            package_of(&info.symbol, &mut candidates);
        }
        let defined: BTreeSet<&str> = out
            .docs
            .values()
            .flat_map(|d| d.defs.iter().map(|def| def.symbol.as_str()))
            .collect();
        out.externs = candidates
            .into_iter()
            .filter(|(symbol, _)| !defined.contains(symbol.as_str()))
            .collect();
        out.recount_collisions();
        out
    }

    /// Recompute [`Self::collisions`] from the documents held now. Called
    /// after every merge of indexes, since a collision can span two.
    pub fn recount_collisions(&mut self) {
        let mut documents: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for (path, doc) in &self.docs {
            for def in &doc.defs {
                documents
                    .entry(def.symbol.as_str())
                    .or_default()
                    .insert(path.as_str());
            }
        }
        self.collisions = documents
            .into_iter()
            .filter(|(_, in_docs)| in_docs.len() > 1)
            .map(|(symbol, _)| symbol.to_string())
            .collect();
    }

    fn document(
        &mut self,
        document: &Document,
        root: &Path,
        candidates: &mut BTreeMap<String, (String, String, String)>,
    ) {
        let path = document.relative_path.replace('\\', "/");
        if escapes_root(&path) {
            self.skipped
                .push((path, "path escapes the repository root"));
            return;
        }
        let encoding = document.position_encoding.enum_value_or_default();
        let Some(columns) = Columns::for_document(document, root, encoding) else {
            self.skipped
                .push((path, "source unavailable for transcoding"));
            return;
        };

        let mut doc = Doc {
            lang: document.language.to_lowercase(),
            ..Doc::default()
        };
        for occurrence in &document.occurrences {
            let Some((line, col, end)) = span_of(occurrence) else {
                continue;
            };
            let symbol = rewrite_local(&occurrence.symbol, &path);
            if symbol.is_empty() {
                continue;
            }
            // A line the source no longer has is a stale range, not a column to
            // approximate; an undeclared column over non-ASCII text is neither
            // placed nor dropped silently (`specs/02-extraction.md` § Position
            // normalization).
            let col = match columns.byte(line, col) {
                Ok(col) => col,
                Err(Unplaced::Ambiguous) => {
                    self.ambiguous += 1;
                    continue;
                }
                Err(Unplaced::Stale) => continue,
            };
            if has(occurrence.symbol_roles, SymbolRole::Definition) {
                doc.defs.push(Def {
                    symbol,
                    line,
                    col,
                    enclosing: enclosing_of(occurrence).and_then(|(sl, sc, el, ec)| {
                        Some((
                            sl,
                            columns.byte(sl, sc).ok()?,
                            el,
                            columns.byte(el, ec).ok()?,
                        ))
                    }),
                });
            } else {
                doc.refs.push(Ref {
                    symbol,
                    line,
                    col,
                    role: role_of(occurrence.symbol_roles),
                });
            }
            let _ = end;
        }

        for info in &document.symbols {
            let symbol = rewrite_local(&info.symbol, &path);
            if symbol.is_empty() {
                continue;
            }
            self.symbols.insert(symbol, info_of(info, &path));
            package_of(&info.symbol, candidates);
        }
        // Every symbol this document *references*, not only the ones it
        // describes. An indexer emits `SymbolInformation` for what the project
        // defines and leaves the rest — `std`, `core`, `alloc` — as bare
        // occurrences, so keying `extern` on `SymbolInformation` reported no
        // external packages at all for a project with no described dependency.
        // The package is in the symbol string either way.
        for occurrence in &doc.refs {
            package_of(&occurrence.symbol, candidates);
        }
        self.docs.insert(path, doc);
    }
}

/// Record a symbol that names a package, which is how SCIP says where a thing
/// came from (`specs/01-facts.md` § `extern`).
///
/// A SCIP symbol carries its own package — `<scheme> <manager> <name>
/// <version> <descriptor>` — so this works on any symbol string and does not
/// need a `SymbolInformation` to exist for it. That matters: an index
/// references far more symbols than it describes. `rust-analyzer` emits
/// `SymbolInformation` for what the project defines and leaves `std`, `core`
/// and `alloc` as bare occurrences, so keying this on `SymbolInformation`
/// meant `extern/4` was **empty on every index that had no local dependency
/// described** — including the fixture, where `HashMap` and `String` are
/// referenced by name and were reported as no external packages at all.
/// Whether a `Document.relative_path` names something outside the repository.
///
/// `relative_path` is specified as relative to the project root, and every fact
/// this tool emits is keyed by a repo-relative path, so a document that climbs
/// out of the root is not a fact about this repository and cannot be joined
/// against one. `scip-go` emits exactly this: the generated test-main for a
/// package with tests lives in the Go build cache, and the document arrives as
/// `../../../../../../Library/Caches/go-build/…`. Ingesting it produced a
/// `file` row outside the tree and, through it, `calls` edges whose caller was
/// a path no reader could open.
///
/// Skipped and **reported** rather than silently dropped, like every other
/// unusable document (`specs/00-overview.md` invariant 5).
fn escapes_root(path: &str) -> bool {
    path.starts_with('/')
        || path.split('/').any(|part| part == "..")
        // A Windows drive or UNC path. `\` is already normalized to `/`.
        || path.split_once(':').is_some_and(|(head, _)| {
            head.len() == 1 && head.chars().all(|c| c.is_ascii_alphabetic())
        })
}

fn package_of(symbol: &str, into: &mut BTreeMap<String, (String, String, String)>) {
    let Ok(parsed) = scip::symbol::parse_symbol(symbol) else {
        return;
    };
    let Some(package) = parsed.package.as_ref() else {
        return;
    };
    if package.name.is_empty() {
        return;
    }
    into.insert(
        symbol.to_string(),
        (
            package.manager.clone(),
            package.name.clone(),
            package.version.clone(),
        ),
    );
}

/// The parent of a symbol, by the fixed precedence in
/// `specs/02-extraction.md` § Parent precedence.
///
/// Descriptor truncation happens on the *parsed* list. A backtick-quoted
/// descriptor name may contain `#`, `.` and `/`, so `rsplit` on the suffix
/// character silently corrupts it — which is the whole reason this is a
/// function and not an expression.
#[must_use]
pub fn owner(symbol: &str) -> Option<String> {
    let mut parsed = scip::symbol::parse_symbol(symbol).ok()?;
    if parsed.descriptors.len() < 2 {
        return None;
    }
    parsed.descriptors.pop();
    Some(scip::symbol::format_symbol(parsed))
}

/// Our `Kind` for a `SymbolInformation.kind`, `unknown` when we have no word
/// for it.
#[must_use]
pub fn kind_of(info: &SymbolInformation) -> &'static str {
    let name = format!("{:?}", info.kind.enum_value_or_default());
    KIND_MAP
        .iter()
        .find_map(|(from, to)| (*from == name).then_some(*to))
        .unwrap_or("unknown")
}

/// `display_name`, else the name of the symbol's final descriptor.
///
/// `scip-python` 0.6.6 writes no `display_name` and no `kind` for a
/// parameter, an attribute, or a module, so a `def` row for `Store#entries.`
/// arrived with `Name = ""`. The descriptor grammar is mandatory and the name
/// is written in it — `entries` — so reading it is extraction, not a guess.
/// The kind stays `unknown`: a `.` descriptor is a field, a variable or a
/// constant, and the suffix does not say which.
fn name_of(info: &SymbolInformation) -> String {
    if !info.display_name.is_empty() {
        return info.display_name.clone();
    }
    scip::symbol::parse_symbol(&info.symbol)
        .ok()
        .and_then(|parsed| parsed.descriptors.last().map(|d| d.name.clone()))
        .unwrap_or_default()
}

fn info_of(info: &SymbolInformation, path: &str) -> Info {
    let doc = info
        .documentation
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    let enclosing = rewrite_local(&info.enclosing_symbol, path);
    Info {
        kind: kind_of(info),
        name: name_of(info),
        doc: (!doc.trim().is_empty()).then(|| doc.trim().to_string()),
        sig: info
            .signature_documentation
            .as_ref()
            .map(|s| s.text.clone())
            .filter(|t| !t.trim().is_empty()),
        enclosing: (!enclosing.is_empty()).then_some(enclosing),
        implements: info
            .relationships
            .iter()
            .filter(|r| r.is_implementation)
            .map(|r| rewrite_local(&r.symbol, path))
            .filter(|s| !s.is_empty())
            .collect(),
    }
}

/// `local N` is document-scoped. Give it its document back.
///
/// Skipping this cross-links every file's `local 4` into one symbol, which is
/// silent and repo-wide (`specs/02-extraction.md` § Local symbols).
#[must_use]
pub fn rewrite_local(symbol: &str, path: &str) -> String {
    match symbol.strip_prefix("local ") {
        Some(rest) => format!("local {path} {rest}"),
        None => symbol.to_string(),
    }
}

/// `(line, start_character, end_character)` from whichever range field the
/// indexer used. 1-based line out, encoding-native column.
fn span_of(occurrence: &Occurrence) -> Option<(u32, u32, u32)> {
    use scip::types::occurrence::Typed_range;
    let (line, start, end) = match occurrence.typed_range.as_ref() {
        Some(Typed_range::SingleLineRange(r)) => (r.line, r.start_character, r.end_character),
        Some(Typed_range::MultiLineRange(r)) => (r.start_line, r.start_character, r.end_character),
        // The deprecated `repeated int32` form. Indexer versions vary, so both
        // are supported (`specs/02-extraction.md` § Position normalization).
        _ => match occurrence.range.as_slice() {
            // Three elements is single-line, four carries an end line we do
            // not need: the column is what a `From` attribution turns on.
            [line, start, end] | [line, start, _, end] => (*line, *start, *end),
            _ => return None,
        },
    };
    Some((line1(line)?, nonneg(start)?, nonneg(end)?))
}

/// The full-definition range, `(start_line, start_col, end_line, end_col)`.
fn enclosing_of(occurrence: &Occurrence) -> Option<(u32, u32, u32, u32)> {
    use scip::types::occurrence::Typed_enclosing_range;
    let (sl, sc, el, ec) = match occurrence.typed_enclosing_range.as_ref() {
        Some(Typed_enclosing_range::SingleLineEnclosingRange(r)) => {
            (r.line, r.start_character, r.line, r.end_character)
        }
        Some(Typed_enclosing_range::MultiLineEnclosingRange(r)) => {
            (r.start_line, r.start_character, r.end_line, r.end_character)
        }
        _ => match occurrence.enclosing_range.as_slice() {
            [sl, sc, ec] => (*sl, *sc, *sl, *ec),
            [sl, sc, el, ec] => (*sl, *sc, *el, *ec),
            _ => return None,
        },
    };
    Some((line1(sl)?, nonneg(sc)?, line1(el)?, nonneg(ec)?))
}

/// The most specific role in the bitset, or `unknown` for an empty one.
///
/// SCIP 0 is `UnspecifiedSymbolRole`, not a read. Reporting it as a read is the
/// extractor answering a question the indexer declined to answer (invariant 1).
fn role_of(bits: i32) -> &'static str {
    ROLES
        .iter()
        .find_map(|(role, name)| has(bits, *role).then_some(*name))
        .unwrap_or("unknown")
}

fn has(bits: i32, role: SymbolRole) -> bool {
    bits & protobuf::Enum::value(&role) != 0
}

/// A 0-based SCIP line as our 1-based one. `None` for a negative line: that is
/// a malformed range, and clamping it to line 1 would put a symbol somewhere it
/// is not (`specs/02-extraction.md` § Position normalization).
fn line1(zero_based: i32) -> Option<u32> {
    u32::try_from(zero_based).ok()?.checked_add(1)
}

/// A column, or `None` if it is negative. Same reasoning as [`line1`].
fn nonneg(n: i32) -> Option<u32> {
    u32::try_from(n).ok()
}

/// Column transcoding for one document.
///
/// Why a SCIP column could not be placed on a byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unplaced {
    /// The line is past the source: the file on disk no longer matches the
    /// index.
    Stale,
    /// The document declares no encoding and the line is not ASCII before the
    /// column, so UTF-8, UTF-16 and UTF-32 put it on different bytes.
    Ambiguous,
}

/// SCIP columns are code units from the line start in the document's own
/// encoding; ours are UTF-8 bytes. For a declared UTF-8 document that is the
/// identity and no source is needed. A declared UTF-16 or UTF-32 document is a
/// per-line scan, and an undeclared one is trusted only over an ASCII prefix.
/// Both need the source, and a document whose source we cannot read is
/// **skipped**, not approximated.
#[derive(Debug)]
enum Columns {
    /// Already ours.
    Identity,
    /// Line text, indexed by 1-based line number, plus the code unit width.
    Transcode { lines: Vec<String>, utf16: bool },
    /// Line text for a document that declares no encoding at all.
    Ascii { lines: Vec<String> },
}

impl Columns {
    fn for_document(document: &Document, root: &Path, encoding: PositionEncoding) -> Option<Self> {
        let utf16 = match encoding {
            PositionEncoding::UTF8CodeUnitOffsetFromLineStart => return Some(Self::Identity),
            // Not "byte-oriented by default". `scip-python` 0.6.6 and
            // `scip-typescript` 0.4.0 declare nothing and count UTF-16 (#21),
            // so an undeclared column is exact only where every encoding
            // agrees on it (`specs/02-extraction.md` § Position normalization).
            PositionEncoding::UnspecifiedPositionEncoding => None,
            PositionEncoding::UTF16CodeUnitOffsetFromLineStart => Some(true),
            PositionEncoding::UTF32CodeUnitOffsetFromLineStart => Some(false),
        };
        let text = if document.text.is_empty() {
            std::fs::read_to_string(root.join(&document.relative_path)).ok()?
        } else {
            document.text.clone()
        };
        let lines = text.lines().map(str::to_string).collect();
        Some(match utf16 {
            Some(utf16) => Self::Transcode { lines, utf16 },
            None => Self::Ascii { lines },
        })
    }

    /// The UTF-8 byte column for a code-unit column on a 1-based line.
    ///
    /// [`Unplaced::Stale`] when the line is past the source — the file on disk
    /// no longer matches the index, and returning the code-unit column
    /// unchanged would emit an approximate column for a UTF-16 document, which
    /// `specs/02-extraction.md` § Position normalization forbids.
    /// [`Unplaced::Ambiguous`] when no encoding was declared and the text
    /// before the column is not ASCII, where the encodings disagree.
    fn byte(&self, line: u32, col: u32) -> std::result::Result<u32, Unplaced> {
        let (lines, utf16) = match self {
            Self::Identity => return Ok(col),
            Self::Transcode { lines, utf16 } => (lines, Some(*utf16)),
            Self::Ascii { lines } => (lines, None),
        };
        let text = usize::try_from(line)
            .ok()
            .and_then(|l| lines.get(l.checked_sub(1)?))
            .ok_or(Unplaced::Stale)?;
        let Some(utf16) = utf16 else {
            // Every encoding counts an ASCII character as one unit, so over an
            // ASCII prefix the column is the same number in all of them.
            let prefix = usize::try_from(col)
                .ok()
                .and_then(|c| text.as_bytes().get(..c));
            return match prefix {
                Some(prefix) if prefix.is_ascii() => Ok(col),
                Some(_) => Err(Unplaced::Ambiguous),
                None => Err(Unplaced::Stale),
            };
        };
        let mut units = 0_u32;
        for (offset, c) in text.char_indices() {
            if units >= col {
                return u32::try_from(offset).ok().ok_or(Unplaced::Stale);
            }
            units = units.saturating_add(if utf16 {
                u32::try_from(c.len_utf16()).unwrap_or(1)
            } else {
                1
            });
        }
        u32::try_from(text.len()).ok().ok_or(Unplaced::Stale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use scip::types::{Symbol, symbol_information::Kind};

    #[test]
    fn a_local_symbol_gets_its_document_back() {
        assert_eq!(
            rewrite_local("local 4", "src/store.rs"),
            "local src/store.rs 4"
        );
        // A global symbol is already unique and must not be touched.
        let global = "rust-analyzer cargo codeintel 0.1.0 store/Store#get().";
        assert_eq!(rewrite_local(global, "src/store.rs"), global);
        assert_eq!(rewrite_local("", "src/store.rs"), "");
    }

    #[test]
    fn the_owner_is_the_symbol_with_its_last_descriptor_dropped() {
        assert_eq!(
            owner("rust-analyzer cargo p 1.0 store/Store#get()."),
            Some("rust-analyzer cargo p 1.0 store/Store#".to_string())
        );
        assert_eq!(
            owner("rust-analyzer cargo p 1.0 store/Store#"),
            Some("rust-analyzer cargo p 1.0 store/".to_string())
        );
        // One descriptor has no owner to name, and a local never had one.
        assert_eq!(owner("rust-analyzer cargo p 1.0 store/"), None);
        assert_eq!(owner("local src/a.rs 4"), None);
    }

    #[test]
    fn a_quoted_descriptor_name_survives_truncation() {
        // The reason this parses rather than rsplits: the name holds every
        // character the suffix grammar uses.
        let symbol = "rust-analyzer cargo p 1.0 `a/b#c.`/get().";
        let parsed = scip::symbol::parse_symbol(symbol).expect("parses");
        assert_eq!(parsed.descriptors.len(), 2);
        assert_eq!(parsed.descriptors[0].name, "a/b#c.");
        assert_eq!(
            owner(symbol),
            Some("rust-analyzer cargo p 1.0 `a/b#c.`/".to_string())
        );
    }

    #[test]
    fn the_role_table_lands_inside_our_vocabulary() {
        // `codeintel schema` prints `lang::ROLES` with a count against each.
        // A role this table can emit but that list does not name would be a
        // value an agent is never told exists.
        for (bit, name) in ROLES {
            assert!(
                crate::lang::ROLES.contains(name),
                "{bit:?} -> {name} is not a Role"
            );
        }
    }

    #[test]
    fn the_kind_table_lands_inside_our_vocabulary() {
        for (from, to) in KIND_MAP {
            assert!(
                crate::lang::KINDS.contains(to),
                "{from} -> {to} is not a Kind"
            );
        }
        let mut info = SymbolInformation::new();
        info.kind = Kind::Struct.into();
        assert_eq!(kind_of(&info), "struct");
        // `Union` is a `type`, distinct from `enum`, and tier A's kind_remap
        // agrees so the anchor join never reconciles a kind.
        info.kind = Kind::Union.into();
        assert_eq!(kind_of(&info), "type");
        info.kind = Kind::Enum.into();
        assert_eq!(kind_of(&info), "enum");
        // A kind we have no word for is `unknown`, not a guess.
        info.kind = Kind::Quasiquoter.into();
        assert_eq!(kind_of(&info), "unknown");
    }

    #[test]
    fn a_role_bitset_reports_its_most_specific_bit() {
        let write = protobuf::Enum::value(&SymbolRole::WriteAccess);
        let read = protobuf::Enum::value(&SymbolRole::ReadAccess);
        assert_eq!(role_of(write), "write");
        assert_eq!(role_of(read), "read");
        assert_eq!(role_of(write | read), "write");
        // No bits set is `UnspecifiedSymbolRole`. Reading it as a read is the
        // extractor guessing; `unknown` is what the indexer actually said.
        assert_eq!(role_of(0), "unknown");
    }

    #[test]
    fn utf16_columns_become_byte_columns() {
        let columns = Columns::Transcode {
            lines: vec!["let 🦀 = crab;".to_string()],
            utf16: true,
        };
        // `🦀` is two UTF-16 units and four UTF-8 bytes, so everything after it
        // shifts. Getting this wrong is a silently wrong edit position.
        assert_eq!(columns.byte(1, 0), Ok(0));
        assert_eq!(columns.byte(1, 4), Ok(4));
        assert_eq!(columns.byte(1, 6), Ok(8));
    }

    #[test]
    fn a_line_past_the_source_is_skipped_not_passed_through() {
        let columns = Columns::Transcode {
            lines: vec!["let 🦀 = crab;".to_string()],
            utf16: true,
        };
        // The file on disk no longer matches the index. Returning the code-unit
        // column unchanged would be an approximate column for a UTF-16
        // document, which `specs/02-extraction.md` forbids.
        assert_eq!(columns.byte(2, 4), Err(Unplaced::Stale));
        assert_eq!(columns.byte(0, 4), Err(Unplaced::Stale));
    }

    #[test]
    fn utf32_columns_become_byte_columns() {
        let columns = Columns::Transcode {
            lines: vec!["let 🦀 = crab;".to_string()],
            utf16: false,
        };
        assert_eq!(columns.byte(1, 5), Ok(8));
    }

    #[test]
    fn an_undeclared_column_is_kept_only_where_every_encoding_agrees() {
        // `s = "é"; x = 1`: `x` is UTF-16 column 9 and byte column 10. With no
        // declared encoding the index could mean either, so a column past the
        // `é` is refused rather than guessed. One before it is the same number
        // in every encoding and is kept (#21).
        let columns = Columns::Ascii {
            lines: vec![r#"s = "é"; x = 1"#.to_string()],
        };
        assert_eq!(columns.byte(1, 0), Ok(0));
        assert_eq!(columns.byte(1, 5), Ok(5));
        assert_eq!(columns.byte(1, 9), Err(Unplaced::Ambiguous));
        assert_eq!(columns.byte(2, 0), Err(Unplaced::Stale));
    }

    #[test]
    fn an_undeclared_document_counts_what_it_could_not_place() {
        let mut document = Document::new();
        document.relative_path = "wide.py".to_string();
        document.text = "s = \"é\"; x = 1\n".to_string();
        for (symbol, start, end) in [
            ("scip-python python p 1.0 wide/s.", 0, 1),
            ("scip-python python p 1.0 wide/x.", 9, 10),
        ] {
            let mut occurrence = Occurrence::new();
            occurrence.symbol = symbol.to_string();
            occurrence.range = vec![0, start, end];
            occurrence.symbol_roles = protobuf::Enum::value(&SymbolRole::Definition);
            document.occurrences.push(occurrence);
        }
        let mut index = Index::new();
        index.documents.push(document);
        let ingest = Ingest::of(&index, Path::new("/nonexistent"));
        let defs: Vec<&str> = ingest
            .docs
            .get("wide.py")
            .expect("ingested from its own text")
            .defs
            .iter()
            .map(|d| d.symbol.as_str())
            .collect();
        assert_eq!(defs, ["scip-python python p 1.0 wide/s."]);
        assert_eq!(ingest.ambiguous, 1);
        assert!(ingest.skipped.is_empty(), "{:?}", ingest.skipped);
    }

    #[test]
    fn an_undeclared_document_with_no_source_is_skipped_not_approximated() {
        // It used to need no source at all, because it was read as UTF-8.
        let mut document = Document::new();
        document.relative_path = "nowhere.py".to_string();
        let mut index = Index::new();
        index.documents.push(document);
        let ingest = Ingest::of(&index, Path::new("/nonexistent"));
        assert!(ingest.docs.is_empty());
        assert_eq!(ingest.skipped.len(), 1, "{:?}", ingest.skipped);
    }

    #[test]
    fn a_utf8_document_needs_no_source() {
        let mut document = Document::new();
        document.relative_path = "nowhere.rs".to_string();
        document.position_encoding = PositionEncoding::UTF8CodeUnitOffsetFromLineStart.into();
        let columns = Columns::for_document(
            &document,
            Path::new("/nonexistent"),
            PositionEncoding::UTF8CodeUnitOffsetFromLineStart,
        )
        .expect("utf-8 needs nothing");
        assert_eq!(columns.byte(9, 42), Ok(42));
    }

    #[test]
    fn a_utf16_document_with_no_source_is_skipped_not_approximated() {
        let mut document = Document::new();
        document.relative_path = "nowhere.rs".to_string();
        document.position_encoding = PositionEncoding::UTF16CodeUnitOffsetFromLineStart.into();
        assert!(
            Columns::for_document(
                &document,
                Path::new("/nonexistent"),
                PositionEncoding::UTF16CodeUnitOffsetFromLineStart,
            )
            .is_none()
        );

        let mut index = Index::new();
        index.documents.push(document);
        let ingest = Ingest::of(&index, Path::new("/nonexistent"));
        assert!(ingest.docs.is_empty());
        assert_eq!(ingest.skipped.len(), 1, "{:?}", ingest.skipped);
    }

    #[test]
    fn both_range_encodings_are_read() {
        let mut typed = Occurrence::new();
        let mut range = scip::types::SingleLineRange::new();
        range.line = 41;
        range.start_character = 4;
        range.end_character = 7;
        typed.typed_range = Some(scip::types::occurrence::Typed_range::SingleLineRange(range));
        assert_eq!(span_of(&typed), Some((42, 4, 7)));

        let mut deprecated = Occurrence::new();
        deprecated.range = vec![41, 4, 7];
        assert_eq!(span_of(&deprecated), Some((42, 4, 7)));

        let mut four = Occurrence::new();
        four.range = vec![41, 4, 43, 7];
        assert_eq!(span_of(&four), Some((42, 4, 7)));

        assert_eq!(span_of(&Occurrence::new()), None);

        // A negative coordinate is a malformed range. Clamping it to line 1
        // would put a symbol at a byte range that is not its own.
        let mut negative = Occurrence::new();
        negative.range = vec![-1, 4, 7];
        assert_eq!(span_of(&negative), None);
        negative.range = vec![41, -4, 7];
        assert_eq!(span_of(&negative), None);
    }

    #[test]
    fn a_symbol_naming_a_package_is_external() {
        let mut info = SymbolInformation::new();
        info.symbol =
            "scip-go gomod github.com/gin-gonic/gin v1.9.1 gin/Context#JSON().".to_string();
        let mut index = Index::new();
        index.external_symbols.push(info.clone());
        let ingest = Ingest::of(&index, Path::new("/nonexistent"));
        assert_eq!(
            ingest.externs.get(&info.symbol),
            Some(&(
                "gomod".to_string(),
                "github.com/gin-gonic/gin".to_string(),
                "v1.9.1".to_string(),
            ))
        );

        // A local names no package, so it is not external.
        let mut local = SymbolInformation::new();
        local.symbol = "local 4".to_string();
        let mut index = Index::new();
        index.external_symbols.push(local);
        assert!(
            Ingest::of(&index, Path::new("/nonexistent"))
                .externs
                .is_empty()
        );
    }

    #[test]
    fn a_whole_document_normalizes() {
        let mut info = SymbolInformation::new();
        info.symbol = "rust-analyzer cargo p 1.0 store/Store#get().".to_string();
        info.display_name = "get".to_string();
        info.kind = Kind::Method.into();
        info.documentation = vec!["Fetch one.".to_string()];

        let mut def = Occurrence::new();
        def.symbol = info.symbol.clone();
        def.range = vec![9, 11, 14];
        def.symbol_roles = protobuf::Enum::value(&SymbolRole::Definition);

        let mut reference = Occurrence::new();
        reference.symbol = "local 4".to_string();
        reference.range = vec![12, 8, 9];

        let mut document = Document::new();
        document.relative_path = "src/store.rs".to_string();
        document.language = "Rust".to_string();
        // What rust-analyzer declares. Undeclared would need the source to
        // tell an ASCII prefix from not, and this test has none.
        document.position_encoding = PositionEncoding::UTF8CodeUnitOffsetFromLineStart.into();
        document.symbols = vec![info];
        document.occurrences = vec![def, reference];

        let mut index = Index::new();
        index.documents.push(document);
        let ingest = Ingest::of(&index, Path::new("/nonexistent"));

        let doc = ingest.docs.get("src/store.rs").expect("the document");
        assert_eq!(doc.lang, "rust");
        assert_eq!(doc.defs.len(), 1);
        assert_eq!(doc.defs[0].line, 10);
        assert_eq!(doc.defs[0].col, 11);
        assert_eq!(doc.refs.len(), 1);
        assert_eq!(doc.refs[0].symbol, "local src/store.rs 4");
        // The occurrence carries no role bits, so the role is `unknown`.
        assert_eq!(doc.refs[0].role, "unknown");

        let symbol = ingest
            .symbols
            .get("rust-analyzer cargo p 1.0 store/Store#get().")
            .expect("the symbol");
        assert_eq!(symbol.kind, "method");
        assert_eq!(symbol.name, "get");
        assert_eq!(symbol.doc.as_deref(), Some("Fetch one."));
    }

    #[test]
    fn a_document_outside_the_repository_is_skipped_and_named() {
        // `scip-go` emits the generated test-main out of the Go build cache,
        // and the document arrives as `../../../../../../Library/Caches/…`.
        // Ingesting it gave `calls` an edge whose caller was a path no reader
        // could open.
        for outside in [
            "../../../../Library/Caches/go-build/9e/9ed0-d",
            "/etc/passwd",
            "a/../../b.go",
            "C:/Users/x/main.go",
        ] {
            assert!(escapes_root(outside), "{outside} escapes the root");
        }
        for inside in ["src/store.rs", "a/b/c.go", "main.go", "..hidden/x.go"] {
            assert!(!escapes_root(inside), "{inside} is inside the root");
        }

        let mut document = Document::new();
        document.relative_path = "../outside.go".to_string();
        document.text = "package p\n".to_string();
        let mut index = Index::new();
        index.documents.push(document);
        let ingest = Ingest::of(&index, Path::new("."));
        assert!(ingest.docs.is_empty(), "nothing outside the root is kept");
        assert_eq!(
            ingest.skipped,
            vec![(
                "../outside.go".to_string(),
                "path escapes the repository root"
            )],
            "and the skip is reported, never silent"
        );
    }

    #[test]
    fn an_unparseable_symbol_is_ignored_rather_than_fatal() {
        drop(scip::symbol::parse_symbol("nonsense").unwrap_err());
        assert_eq!(owner("nonsense"), None);
        let _ = Symbol::new();
    }
}
