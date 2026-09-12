//! What this index actually holds, counted.
//!
//! Both `schema` and `status` are views of the same numbers, and the numbers
//! are **observed, never declared**. A static catalogue advertising sixteen
//! kinds when the index holds four is this project's own invariant 5 broken by
//! its own onboarding text (`specs/05-surface.md` § `schema`), and
//! "python: 1,204 files, 11 defs" is visibly absurd to a human in one second
//! where `status: ok` is not.

use std::collections::BTreeMap;
use std::fmt;

use anyhow::{Context, Result};
use datalog::Relation;
use datalog::atom::Atom;
use facts::{Manifest, Store};

/// One language's share of the index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LangCount {
    /// Files indexed in this language.
    pub files: usize,
    /// `def` rows in those files.
    pub defs: usize,
    /// `scip_ref` rows in those files — compiler-resolved occurrences.
    ///
    /// Kept apart from [`Self::named`] rather than summed. A single `refs`
    /// column cannot tell a reader whether 200 of 12,000 are anchored or
    /// 11,000 are, and M3 measured tier A over-reporting 17% of the call edges
    /// it claims. The ratio is the number that decides whether to trust an
    /// answer, so `status` must not make you ask for it.
    pub exact: usize,
    /// `name_ref` rows in those files — identifier matches, tier A.
    pub named: usize,
    /// `import` rows in those files.
    pub imports: usize,
}

/// The counts behind `schema` and `status`.
#[derive(Debug, Clone, Default)]
pub struct Census {
    /// False when there is no `.codeintel/` here. Every count is then zero,
    /// and the two cases must not print the same.
    pub indexed: bool,
    /// Rows per base relation, for every relation in the schema — including
    /// the ones at zero, which are the ones worth seeing.
    pub relations: BTreeMap<&'static str, usize>,
    /// Per language, keyed by the manifest's language name and printed in that
    /// key order.
    pub langs: BTreeMap<String, LangCount>,
    /// `def` rows per `Kind`. A kind at zero is a kind not to query.
    pub kinds: BTreeMap<String, usize>,
    /// `scip_ref` rows per `Role`. Tier A emits no role.
    pub roles: BTreeMap<String, usize>,
    /// Files whose `mtime`/`size` no longer match the manifest.
    pub changed: Vec<String>,
    /// Extensions found in the tree with no grammar, and how many files each.
    ///
    /// A silently unindexed subtree is the most confusing failure this tool can
    /// have, and it is invisible from the index alone — the files are simply
    /// not in it. Filling this costs one walk, which is why it happens in
    /// `status` and not on every query.
    pub unsupported: BTreeMap<String, usize>,
}

impl Census {
    /// Count an index. A missing one is a census with `indexed: false`, not an
    /// error — "no index here" is an answer.
    ///
    /// # Errors
    /// A store that exists but cannot be read.
    pub fn of(store: &Store) -> Result<Self> {
        let mut census = Self {
            indexed: store.has_index(),
            ..Self::default()
        };
        // Every relation, present or absent: a relation at zero is a fact
        // about this index, not an omission.
        for rel in facts::RELATIONS {
            census.relations.insert(rel.name, 0);
        }
        if !census.indexed {
            return Ok(census);
        }

        let manifest = store.manifest();
        let relations = store.load().context("loading the index")?;
        for (name, rows) in &relations {
            census.relations.insert(name, rows.len());
        }

        // File -> language, as the manifest records it. Going through the
        // manifest rather than `file(F, Lang)` keeps this working when the
        // relation is empty but the file table is not.
        let lang_of: BTreeMap<&str, &str> = manifest
            .files
            .iter()
            .map(|(path, entry)| (path.as_str(), entry.lang.as_str()))
            .collect();
        for lang in lang_of.values() {
            census.langs.entry((*lang).to_string()).or_default().files += 1;
        }

        let file_atom = |atom: Atom| -> Option<&str> { store.resolve(atom) };
        let mut attribute = |rows: Option<&Relation>, column: usize, pick: fn(&mut LangCount)| {
            let Some(rows) = rows else { return };
            for row in rows.iter() {
                let Some(lang) = row
                    .get(column)
                    .copied()
                    .and_then(file_atom)
                    .and_then(|path| lang_of.get(path))
                else {
                    continue;
                };
                pick(census.langs.entry((*lang).to_string()).or_default());
            }
        };
        attribute(relations.get("def"), 1, |c| c.defs += 1);
        attribute(relations.get("scip_ref"), 1, |c| c.exact += 1);
        attribute(relations.get("name_ref"), 1, |c| c.named += 1);
        attribute(relations.get("import"), 0, |c| c.imports += 1);

        // def(S, F, Kind, Name) — the closed set as it actually occurs.
        if let Some(defs) = relations.get("def") {
            for row in defs.iter() {
                if let Some(kind) = row.get(2).copied().and_then(|a| store.resolve(a)) {
                    *census.kinds.entry(kind.to_string()).or_default() += 1;
                }
            }
        }
        // scip_ref(S, F, Line, Col, From, Role)
        if let Some(refs) = relations.get("scip_ref") {
            for row in refs.iter() {
                if let Some(role) = row.get(5).copied().and_then(|a| store.resolve(a)) {
                    *census.roles.entry(role.to_string()).or_default() += 1;
                }
            }
        }

        census.changed = changed_files(store.root(), manifest);
        Ok(census)
    }

    /// Walk the tree and record the extensions no grammar covers.
    ///
    /// Separate from [`Self::of`] because it costs a full gitignore-aware
    /// traversal: `schema` does not need it and `status` does.
    ///
    /// With no index the walk is skipped. There is nothing to report an
    /// unindexed extension *against* — the answer is three lines of `no-index`
    /// — and the guard lives here rather than at the call site so a second
    /// caller cannot forget it.
    #[must_use]
    pub fn with_unsupported(mut self, root: &std::path::Path) -> Self {
        if !self.indexed {
            return self;
        }
        let (_, skips) = extract::walk::walk(root);
        self.unsupported = skips.unsupported;
        self
    }

    /// Rows in one relation, or zero.
    #[must_use]
    pub fn rows(&self, relation: &str) -> usize {
        self.relations.get(relation).copied().unwrap_or_default()
    }

    /// Indexed files, from the language table.
    #[must_use]
    pub fn files(&self) -> usize {
        self.langs.values().map(|c| c.files).sum()
    }

    /// Share of definitions the anchor join reached, as a percentage, or
    /// `None` when there are no definitions to divide by.
    #[must_use]
    pub fn anchor_rate(&self) -> Option<f64> {
        let defs = self.rows("def");
        if defs == 0 {
            return None;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a percentage printed to one decimal"
        )]
        let rate = self.rows("resolved") as f64 * 100.0 / defs as f64;
        Some(rate)
    }
}

/// `codeintel status` as text, for a human deciding whether to trust an answer.
///
/// A `Display` over borrowed state rather than a function returning `String`:
/// every `write!` carries its own error instead of one being discarded at each
/// of a dozen call sites.
#[derive(Debug, Clone, Copy)]
pub struct Report<'a> {
    /// What the index holds.
    pub census: &'a Census,
    /// What wrote it.
    pub manifest: &'a Manifest,
}

impl Report<'_> {
    /// The one-word verdict, shared by both formats so they cannot disagree.
    #[must_use]
    pub fn status(&self) -> &'static str {
        if !self.census.indexed {
            "no-index"
        } else if self.census.changed.is_empty() {
            "ok"
        } else {
            "stale"
        }
    }
}

impl fmt::Display for Report<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (census, manifest) = (self.census, self.manifest);
        if !census.indexed {
            return f.write_str("status: no-index\nrun: codeintel index .\n");
        }
        writeln!(f, "status: {}", self.status())?;
        writeln!(f, "root: {}", manifest.roots.join(", "))?;
        writeln!(
            f,
            "written by: {} (schema {}, dict generation {})",
            manifest.writer_version, manifest.schema_version, manifest.dict_generation
        )?;
        // The fingerprint is what makes a bug report reproducible: two indexes
        // of the same tree that disagree, disagree because of this.
        writeln!(f, "extractor: {}", manifest.extractor_fingerprint)?;
        writeln!(f, "indexed: {} files", census.files())?;

        for (lang, counts) in &census.langs {
            writeln!(
                f,
                "  {lang:<12} {} files, {} defs, {} exact refs, {} name refs, {} imports",
                counts.files, counts.defs, counts.exact, counts.named, counts.imports
            )?;
        }
        if !census.unsupported.is_empty() {
            // Biggest first: the number that matters is the one that turns out
            // to be a whole unindexed subtree.
            let mut by_size: Vec<(&String, &usize)> = census.unsupported.iter().collect();
            by_size.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
            let named: Vec<String> = by_size
                .iter()
                .map(|(ext, count)| format!("{count} {ext}"))
                .collect();
            writeln!(f, "unsupported: {}", named.join(", "))?;
        }

        if census.changed.is_empty() {
            f.write_str("changed since index: none\n")?;
        } else {
            let mut named: Vec<&str> = census.changed.iter().map(String::as_str).take(5).collect();
            if census.changed.len() > named.len() {
                named.push("...");
            }
            writeln!(
                f,
                "changed since index: {} — {}",
                census.changed.len(),
                named.join(", ")
            )?;
        }

        if manifest.scip.is_empty() {
            f.write_str(
                "scip: none. every reference is provenance \"name\", so calls may be\n      \
                 wrong and calls_exact is empty.\n",
            )?;
        } else {
            for input in &manifest.scip {
                writeln!(
                    f,
                    "scip: {} ({}), {} documents",
                    input.path,
                    if input.tool.is_empty() {
                        "unknown tool"
                    } else {
                        &input.tool
                    },
                    input.documents
                )?;
            }
            if let Some(rate) = census.anchor_rate() {
                writeln!(
                    f,
                    "anchored: {}/{} defs ({rate:.1}%)",
                    census.rows("resolved"),
                    census.rows("def")
                )?;
            }
        }

        f.write_str("facts:\n")?;
        for (name, count) in &census.relations {
            writeln!(f, "  {name:<12} {count}")?;
        }
        Ok(())
    }
}

impl Report<'_> {
    /// `codeintel status --format json` — the bug-report artifact for a tool
    /// with no telemetry.
    ///
    /// Carries the extractor fingerprint and per-language fact counts, because
    /// "python: 1,204 files, 11 defs" is visibly absurd to a human in one
    /// second and `status: ok` is not (`docs/plan.md` M4).
    ///
    /// The verdict comes from [`Self::status`], the same call the text form
    /// makes, so the two formats cannot disagree about whether the index is
    /// stale.
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        let (census, manifest) = (self.census, self.manifest);
        serde_json::json!({
        "status": self.status(),
        "indexed": census.indexed,
        "roots": manifest.roots,
        "writer_version": manifest.writer_version,
        "schema_version": manifest.schema_version,
        "extractor_fingerprint": manifest.extractor_fingerprint,
        "dict_generation": manifest.dict_generation,
        "files": census.files(),
        "languages": census.langs.iter().map(|(lang, counts)| {
            (lang.clone(), serde_json::json!({
                "files": counts.files,
                "defs": counts.defs,
                "refs_exact": counts.exact,
                "refs_name": counts.named,
                "imports": counts.imports,
            }))
        }).collect::<serde_json::Map<_, _>>(),
        "unsupported": census.unsupported,
        "changed": census.changed,
        "relations": census.relations,
        "kinds": census.kinds,
        "roles": census.roles,
        "scip": manifest.scip.iter().map(|input| serde_json::json!({
            "path": input.path,
            "tool": input.tool,
            "documents": input.documents,
            "mtime": input.mtime,
            "size": input.size,
        })).collect::<Vec<_>>(),
        "anchor_rate": census.anchor_rate(),
        })
    }
}

/// Files the manifest lists whose `mtime` or `size` no longer match — what an
/// auto-refreshing `query` would re-extract, and what makes an answer stale.
fn changed_files(root: &std::path::Path, manifest: &Manifest) -> Vec<String> {
    let mut out = Vec::new();
    for (path, entry) in &manifest.files {
        let Ok(meta) = std::fs::metadata(root.join(path)) else {
            // Gone is changed: its facts are still answering queries.
            out.push(path.clone());
            continue;
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs());
        if !entry.looks_unchanged(mtime, meta.len()) {
            out.push(path.clone());
        }
    }
    out
}
