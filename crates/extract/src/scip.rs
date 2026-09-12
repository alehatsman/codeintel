//! Reading `index.scip` into something per-file.
//!
//! Everything here is normalization, not interpretation: positions become our
//! convention, SCIP's 80-odd kinds collapse onto our sixteen, document-scoped
//! `local N` symbols get their document back, and descriptors are *parsed*
//! rather than sliced. What the facts then say is [`crate::tier_b`]'s job.
//!
//! `specs/02-extraction.md` § Ingest is the contract, field for field.

use std::collections::BTreeMap;
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
const KIND_MAP: &[(&str, &str)] = &[
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
    /// Documents skipped, with the reason. Never silent: an approximate column
    /// breaks every edit built on it, so a document we cannot place exactly is
    /// dropped and reported (`specs/02-extraction.md` § Position normalization).
    pub skipped: Vec<(String, &'static str)>,
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
            package_of(info, &mut candidates);
        }
        let defined: std::collections::BTreeSet<&str> = out
            .docs
            .values()
            .flat_map(|d| d.defs.iter().map(|def| def.symbol.as_str()))
            .collect();
        out.externs = candidates
            .into_iter()
            .filter(|(symbol, _)| !defined.contains(symbol.as_str()))
            .collect();
        out
    }

    fn document(
        &mut self,
        document: &Document,
        root: &Path,
        candidates: &mut BTreeMap<String, (String, String, String)>,
    ) {
        let path = document.relative_path.replace('\\', "/");
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
            let col = columns.byte(line, col);
            if has(occurrence.symbol_roles, SymbolRole::Definition) {
                doc.defs.push(Def {
                    symbol,
                    line,
                    col,
                    enclosing: enclosing_of(occurrence).map(|(sl, sc, el, ec)| {
                        (sl, columns.byte(sl, sc), el, columns.byte(el, ec))
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
            package_of(info, candidates);
        }
        self.docs.insert(path, doc);
    }
}

/// Record a symbol that names a package, which is how SCIP says where a thing
/// came from (`specs/01-facts.md` § `extern`).
fn package_of(info: &SymbolInformation, into: &mut BTreeMap<String, (String, String, String)>) {
    let Ok(parsed) = scip::symbol::parse_symbol(&info.symbol) else {
        return;
    };
    let Some(package) = parsed.package.as_ref() else {
        return;
    };
    if package.name.is_empty() {
        return;
    }
    into.insert(
        info.symbol.clone(),
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
        name: info.display_name.clone(),
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
    Some((line1(line), nonneg(start), nonneg(end)))
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
    Some((line1(sl), nonneg(sc), line1(el), nonneg(ec)))
}

fn role_of(bits: i32) -> &'static str {
    ROLES
        .iter()
        .find_map(|(role, name)| has(bits, *role).then_some(*name))
        .unwrap_or("read")
}

fn has(bits: i32, role: SymbolRole) -> bool {
    bits & protobuf::Enum::value(&role) != 0
}

fn line1(zero_based: i32) -> u32 {
    u32::try_from(zero_based).unwrap_or(0).saturating_add(1)
}

fn nonneg(n: i32) -> u32 {
    u32::try_from(n).unwrap_or(0)
}

/// Column transcoding for one document.
///
/// SCIP columns are code units from the line start in the document's own
/// encoding; ours are UTF-8 bytes. For a UTF-8 document that is the identity
/// and no source is needed. For anything else it is a per-line scan, and a
/// document whose source we cannot read is **skipped**, not approximated.
#[derive(Debug)]
enum Columns {
    /// Already ours.
    Identity,
    /// Line text, indexed by 1-based line number, plus the code unit width.
    Transcode { lines: Vec<String>, utf16: bool },
}

impl Columns {
    fn for_document(document: &Document, root: &Path, encoding: PositionEncoding) -> Option<Self> {
        let utf16 = match encoding {
            // `UnspecifiedPositionEncoding` is what indexers that predate the
            // field emit, and every one of them is byte-oriented. Treating it
            // as UTF-16 would move every column on every non-ASCII line.
            PositionEncoding::UTF8CodeUnitOffsetFromLineStart
            | PositionEncoding::UnspecifiedPositionEncoding => return Some(Self::Identity),
            PositionEncoding::UTF16CodeUnitOffsetFromLineStart => true,
            PositionEncoding::UTF32CodeUnitOffsetFromLineStart => false,
        };
        let text = if document.text.is_empty() {
            std::fs::read_to_string(root.join(&document.relative_path)).ok()?
        } else {
            document.text.clone()
        };
        Some(Self::Transcode {
            lines: text.lines().map(str::to_string).collect(),
            utf16,
        })
    }

    /// The UTF-8 byte column for a code-unit column on a 1-based line.
    fn byte(&self, line: u32, col: u32) -> u32 {
        let (lines, utf16) = match self {
            Self::Identity => return col,
            Self::Transcode { lines, utf16 } => (lines, *utf16),
        };
        let Some(text) = usize::try_from(line)
            .ok()
            .and_then(|l| lines.get(l.saturating_sub(1)))
        else {
            return col;
        };
        let mut units = 0_u32;
        for (offset, c) in text.char_indices() {
            if units >= col {
                return u32::try_from(offset).unwrap_or(col);
            }
            units = units.saturating_add(if utf16 {
                u32::try_from(c.len_utf16()).unwrap_or(1)
            } else {
                1
            });
        }
        u32::try_from(text.len()).unwrap_or(col)
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
        // No bits set is a plain read, which is what indexers emit by default.
        assert_eq!(role_of(0), "read");
    }

    #[test]
    fn utf16_columns_become_byte_columns() {
        let columns = Columns::Transcode {
            lines: vec!["let 🦀 = crab;".to_string()],
            utf16: true,
        };
        // `🦀` is two UTF-16 units and four UTF-8 bytes, so everything after it
        // shifts. Getting this wrong is a silently wrong edit position.
        assert_eq!(columns.byte(1, 0), 0);
        assert_eq!(columns.byte(1, 4), 4);
        assert_eq!(columns.byte(1, 6), 8);
    }

    #[test]
    fn utf32_columns_become_byte_columns() {
        let columns = Columns::Transcode {
            lines: vec!["let 🦀 = crab;".to_string()],
            utf16: false,
        };
        assert_eq!(columns.byte(1, 5), 8);
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
        assert_eq!(columns.byte(9, 42), 42);
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
        assert_eq!(doc.refs[0].role, "read");

        let symbol = ingest
            .symbols
            .get("rust-analyzer cargo p 1.0 store/Store#get().")
            .expect("the symbol");
        assert_eq!(symbol.kind, "method");
        assert_eq!(symbol.name, "get");
        assert_eq!(symbol.doc.as_deref(), Some("Fetch one."));
    }

    #[test]
    fn an_unparseable_symbol_is_ignored_rather_than_fatal() {
        drop(scip::symbol::parse_symbol("nonsense").unwrap_err());
        assert_eq!(owner("nonsense"), None);
        let _ = Symbol::new();
    }
}
