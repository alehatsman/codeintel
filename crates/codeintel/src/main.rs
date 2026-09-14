//! Entry point. Parses arguments, calls into the library, maps a status to an
//! exit code — and nothing else.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use codeintel::Status;
use codeintel::census::{Census, Report};
use codeintel::index::{self, Plan};
use codeintel::query::{self, Options};
use codeintel::schema::{self, Schema};
use codeintel::wire::{self, Wire};
use facts::{Lock, Store};

/// Query structural facts about a source tree with Datalog.
#[derive(Debug, Parser)]
#[command(name = "codeintel", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The verbs. The budget is five (`specs/00-overview.md` § Surface budget) and
/// all five are here: growing to six requires deleting one.
#[derive(Debug, Subcommand)]
enum Command {
    /// Build or update the index.
    Index {
        /// The repository root.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Discard and rewrite everything, dictionary included.
        #[arg(long)]
        rebuild: bool,
        /// Restrict to these languages. Repeatable.
        #[arg(long = "lang")]
        langs: Vec<String>,
        /// A SCIP index to ingest. Repeatable. Defaults to `./index.scip` if
        /// one is there.
        #[arg(long = "scip")]
        scip: Vec<PathBuf>,
    },
    /// Evaluate a Datalog program against the index.
    Query {
        /// The program, or `-` to read it from stdin.
        program: String,
        /// The repository root.
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
        /// Rows before truncation.
        #[arg(long)]
        limit: Option<usize>,
        /// Skip auto-refresh.
        #[arg(long)]
        no_refresh: bool,
        /// Print the underlying atoms instead of `Name path:line`. Use it when
        /// piping one query's output into another query's literal.
        #[arg(long)]
        raw: bool,
        /// A file of extra Datalog rules, loaded after the standard library.
        /// Repeatable. This is where a repository keeps its conformance rules.
        #[arg(long = "rules")]
        rules: Vec<PathBuf>,
        /// Exit 1 if the query returns any row. A conformance check states the
        /// violation it looks for, so finding none is the passing case.
        #[arg(long)]
        expect_empty: bool,
    },
    /// Print the relation catalog, the value vocabularies and the rules.
    Schema {
        /// The repository root, whose index supplies the counts.
        #[arg(long, default_value = ".")]
        path: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
    /// Serve the one MCP tool on stdin and stdout.
    Mcp {
        /// The repository root.
        #[arg(long, default_value = ".")]
        path: PathBuf,
    },
    /// Report what the index holds and how far it has drifted from the tree.
    Status {
        /// The repository root.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Output format.
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
    },
}

/// How to print an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    /// One row per line, tab separated.
    Text,
    /// The response contract from `specs/05-surface.md`.
    Json,
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        // 2, not 1: 1 is `--expect-empty`'s "ran and found a violation", and
        // CI must not read "could not run" as that (`specs/05-surface.md`
        // § Status taxonomy).
        Err(e) => {
            eprintln!("codeintel: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<ExitCode> {
    match Cli::parse().command {
        Command::Index {
            path,
            rebuild,
            langs,
            scip,
        } => index_cmd(&path, rebuild, langs, scip),
        Command::Query {
            program,
            path,
            format,
            limit,
            no_refresh,
            raw,
            rules,
            expect_empty,
        } => query_cmd(
            &program,
            &path,
            format,
            &Options {
                limit: limit.unwrap_or_else(|| Options::default().limit),
                no_refresh,
                raw,
                rules,
                wire: match format {
                    Format::Text => Wire::Text,
                    Format::Json => Wire::Json,
                },
            },
            expect_empty,
        ),
        Command::Schema { path, format } => schema_cmd(&path, format),
        Command::Status { path, format } => status_cmd(&path, format),
        Command::Mcp { path } => mcp_cmd(&path),
    }
}

/// The catalog an agent reads to learn the system. Counts come from the index
/// when there is one; with none, every count is zero and the output says so
/// rather than printing a static list that looks like an inventory.
fn schema_cmd(path: &std::path::Path, format: Format) -> Result<ExitCode> {
    let (store, _) = open(path)?;
    let census = Census::of(&store)?;
    let schema = Schema { census: &census };
    match format {
        Format::Text => print!("{schema}"),
        // The counts go in structurally as well as inside `text`. The whole
        // argument for the text form is that a value at 0 is a value not to
        // query — and a consumer that has to regex prose to learn that is the
        // same defect `query` has when `display` is the only thing on offer.
        Format::Json => println!(
            "{}",
            serde_json::json!({
                "text": schema.to_string(),
                "indexed": census.indexed,
                "rules": schema::rules(schema::STDLIB)
                    .iter()
                    .map(|r| serde_json::json!({ "head": r.head, "doc": r.doc }))
                    .collect::<Vec<_>>(),
                "relations": facts::RELATIONS
                    .iter()
                    .map(|rel| serde_json::json!({
                        "name": rel.name,
                        "args": schema::signature(rel.name).0,
                        "rows": census.rows(rel.name),
                    }))
                    .collect::<Vec<_>>(),
                "kinds": counted(extract::lang::KINDS, &census.kinds),
                "roles": counted(extract::lang::ROLES, &census.roles),
            })
        ),
    }
    Ok(ExitCode::SUCCESS)
}

/// A closed vocabulary with this index's count against each value, zeros
/// included — a value at 0 is a value not to query.
fn counted(
    vocabulary: &[&str],
    counts: &std::collections::BTreeMap<String, usize>,
) -> serde_json::Value {
    serde_json::Value::Object(
        vocabulary
            .iter()
            .map(|name| {
                let n = counts.get(*name).copied().unwrap_or(0);
                ((*name).to_string(), serde_json::json!(n))
            })
            .collect(),
    )
}

/// Index freshness and per-language counts. `--format json` is the bug-report
/// artifact for a tool with no telemetry.
fn status_cmd(path: &std::path::Path, format: Format) -> Result<ExitCode> {
    let root = path
        .canonicalize()
        .with_context(|| format!("{} does not exist", path.display()))?;
    // `query`'s checks, in `query`'s order (`specs/05-surface.md` § `status`).
    let store = match Store::open(&root, &extract::fingerprint()) {
        Ok(store) => store,
        Err(e) => return refusal(e, format),
    };
    if store.has_index()
        && let Some(hint) = query::stale_schema(store.manifest(), &root)
    {
        return Ok(verdict(Status::Stale, &hint, format));
    }
    let census = match Census::of(&store) {
        Ok(census) => census.with_tree(&root, store.manifest()),
        Err(e) => return refusal(e, format),
    };
    let report = Report {
        census: &census,
        manifest: store.manifest(),
    };
    match format {
        Format::Text => print!("{report}"),
        Format::Json => println!("{}", report.json()),
    }
    // A missing index is a fact about this directory, not a failure of the
    // command that reported it.
    Ok(ExitCode::SUCCESS)
}

/// A `status` verdict with no counts behind it: the store would not load, or
/// was written for another schema. Exits as `query` would.
fn verdict(status: Status, hint: &str, format: Format) -> ExitCode {
    match format {
        Format::Text => print!("status: {status}\nhint: {hint}\n"),
        Format::Json => println!(
            "{}",
            serde_json::json!({ "status": status.as_str(), "indexed": true, "hint": hint })
        ),
    }
    if status.answered() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    }
}

/// What the store refused, as a verdict; an I/O failure, as the error it is.
fn refusal(error: std::io::Error, format: Format) -> Result<ExitCode> {
    match facts::fault(&error) {
        Some(fault) => Ok(verdict(Status::of_fault(fault), &error.to_string(), format)),
        None => Err(anyhow::Error::new(error).context("opening the index")),
    }
}

/// The one MCP tool, over JSON-RPC on stdin and stdout.
fn mcp_cmd(path: &std::path::Path) -> Result<ExitCode> {
    let root = path
        .canonicalize()
        .with_context(|| format!("{} does not exist", path.display()))?;
    let stdin = std::io::stdin();
    codeintel::mcp::serve(&root, stdin.lock(), std::io::stdout().lock())?;
    Ok(ExitCode::SUCCESS)
}

/// Open the store under `path`, returning it and the canonical root.
fn open(path: &std::path::Path) -> Result<(Store, PathBuf)> {
    let root = path
        .canonicalize()
        .with_context(|| format!("{} does not exist", path.display()))?;
    let store = Store::open(&root, &extract::fingerprint()).context("opening the index")?;
    Ok((store, root))
}

fn index_cmd(
    path: &std::path::Path,
    rebuild: bool,
    langs: Vec<String>,
    scip: Vec<PathBuf>,
) -> Result<ExitCode> {
    let root = path
        .canonicalize()
        .with_context(|| format!("{} does not exist", path.display()))?;
    if index::ignore_the_store(&root)? {
        eprintln!("codeintel: added `.codeintel/` to .gitignore — the store is derived");
    }

    let mut lock = Lock::open(&root.join(facts::store::DIR)).context("opening the writer lock")?;
    let held = match lock.try_hold().context("taking the writer lock")? {
        Ok(held) => held,
        Err(contended) => {
            // `locked` is for a second writer, and this is one.
            eprintln!(
                "codeintel: status=locked — another index run holds .codeintel/lock{}",
                contended
                    .pid
                    .map_or_else(String::new, |pid| format!(" (pid {pid})"))
            );
            return Ok(ExitCode::from(2));
        }
    };

    // Under the lock, not before it: a concurrent auto-refresh that took the
    // lock first would otherwise write into the directory being deleted.
    let mut generation = 0;
    if rebuild {
        generation = index::discard(&root)?;
    }
    let mut store = Store::open(&root, &extract::fingerprint()).context("opening the index")?;
    if rebuild {
        store.manifest_mut().dict_generation = generation;
    }
    // `--scip` if given, else `./index.scip` when it is there. Never implicit
    // beyond that: nothing is executed, only read.
    let scip = if scip.is_empty() {
        let default = root.join(index::DEFAULT_SCIP);
        if default.exists() {
            vec![PathBuf::from(index::DEFAULT_SCIP)]
        } else {
            Vec::new()
        }
    } else {
        scip
    };
    let report = index::refresh(
        &mut store,
        &Plan {
            rebuild,
            langs,
            deadline: None,
            scip,
        },
    )?;
    drop(held);

    // Summary to stderr, so a piped query is not polluted by it.
    eprintln!(
        "codeintel: {} indexed, {} unchanged, {} removed in {} ms",
        report.indexed, report.unchanged, report.removed, report.elapsed_ms
    );
    eprintln!(
        "  facts: {} defs, {} refs, {} imports",
        report.counts.defs, report.counts.refs, report.counts.imports
    );
    let mut skipped = Vec::new();
    if report.binary > 0 {
        skipped.push(format!("{} not UTF-8", report.binary));
    }
    if report.skips.too_large > 0 {
        skipped.push(format!("{} too large", report.skips.too_large));
    }
    for (ext, count) in &report.skips.unsupported {
        skipped.push(format!("{count} unsupported ({ext})"));
    }
    if !report.skips.unreadable.is_empty() {
        skipped.push(format!("{} unreadable", report.skips.unreadable.len()));
    }
    if !skipped.is_empty() {
        // By reason, never a bare total: a silently unindexed subtree is the
        // single most confusing failure this tool can have.
        eprintln!("  skipped: {}", skipped.join(", "));
    }
    report_scip(&report);
    Ok(ExitCode::SUCCESS)
}

/// The tier-B half of the summary: what was ingested, how well it joined, and
/// — when nothing was — the exact command that would close the gap.
fn report_scip(report: &index::Report) {
    if report.scip.is_empty() {
        let mut langs: Vec<&str> = report
            .langs
            .iter()
            .filter_map(|name| extract::lang::by_name(name).map(|l| l.indexer))
            .collect();
        langs.sort_unstable();
        langs.dedup();
        if langs.is_empty() {
            eprintln!("  no SCIP index: tier A only, so every reference is provenance \"name\"");
            return;
        }
        // The literal line, not a description of one.
        eprintln!(
            "  no SCIP index: tier A only, so every reference is provenance \"name\". build one:"
        );
        for command in langs {
            eprintln!("    {command}");
        }
        return;
    }

    for input in &report.scip {
        eprintln!(
            "  scip: {} ({}) — {} documents",
            input.path,
            if input.tool.is_empty() {
                "unknown tool"
            } else {
                &input.tool
            },
            input.documents
        );
    }
    eprintln!(
        "  tier B: {} refs, {} resolved, {} defs tier A missed",
        report.scip_counts.refs, report.scip_counts.resolved, report.scip_counts.only
    );
    let collisions = report.scip_collisions;
    if collisions > 0 {
        eprintln!(
            "  scip collisions: {collisions} symbol(s) defined in more than one document were \
             not adopted ({} definition(s) keep tier-A identity)",
            report.scip_counts.collided
        );
    }
    let ambiguous: u64 = report.scip.iter().map(|i| i.ambiguous).sum();
    if ambiguous > 0 {
        eprintln!(
            "  scip ambiguous: {ambiguous} occurrence(s) skipped; the index declares no position \
             encoding and the line is not ASCII before the column"
        );
    }
    if let Some(rate) = report.anchor_rate() {
        eprintln!(
            "  anchored: {}/{} defs ({rate:.1}%)",
            report.anchored, report.counts.defs
        );
    }
    if !report.scip_skipped.is_empty() {
        // Never silent: an approximate column breaks every edit built on it.
        eprintln!("  scip skipped {} document(s):", report.scip_skipped.len());
        for (path, why) in report.scip_skipped.iter().take(5) {
            eprintln!("    {path}: {why}");
        }
    }
    if !report.scip_stale.is_empty() {
        eprintln!(
            "  status=scip-stale — {} file(s) changed after the SCIP index was built; their \
             `name_ref` rows are fresh and their `scip_ref` rows are not",
            report.scip_stale.len()
        );
    }
}

fn query_cmd(
    program: &str,
    path: &std::path::Path,
    format: Format,
    options: &Options,
    expect_empty: bool,
) -> Result<ExitCode> {
    let source = if program == "-" {
        let mut buffer = String::new();
        std::io::stdin()
            .read_to_string(&mut buffer)
            .context("reading the program from stdin")?;
        buffer
    } else {
        program.to_string()
    };

    let answer = query::run(path, &source, options)?;

    match format {
        Format::Json => println!("{}", wire::json(&answer)),
        Format::Text => {
            // A ground goal has no columns: truth is one empty row and
            // falsehood is none (`specs/03-datalog.md` § Evaluation). Printed
            // literally that is a bare newline versus nothing — an answer no
            // one can see. Say it.
            if answer.columns.is_empty() && !answer.rows.is_empty() {
                println!("true");
            } else {
                for row in &answer.rows {
                    println!("{}", row.line(options.raw));
                }
            }
            // The status goes to stderr so that stdout is exactly the rows —
            // but it is never omitted, because `ok` with zero rows and a
            // missing index must be distinguishable without reading prose.
            eprintln!("status={}", answer.status);
            if let Some(hint) = &answer.hint {
                eprintln!("hint: {hint}");
            }
        }
    }
    // Never silent: a repository rule replacing `is_test` changes every answer
    // that reads it, and the caller has to know it happened.
    for head in &answer.shadowed {
        eprintln!("shadowed: `{head}` from a rule file replaces the stdlib rule");
    }

    if !answer.status.answered() {
        return Ok(ExitCode::from(2));
    }
    // A conformance check states the violation it looks for, so finding none is
    // the passing case. Exit 1 — distinct from the 2 that means the query never
    // ran, because "your code violates this" and "I could not tell you" are not
    // the same result in CI.
    if expect_empty && !answer.rows.is_empty() {
        eprintln!(
            "expect-empty: {} row(s) returned; this check states a violation and found one",
            answer.rows.len()
        );
        return Ok(ExitCode::from(1));
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn the_cli_is_well_formed() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_verb_budget_holds() {
        // `specs/00-overview.md` § Surface budget: five verbs, and growing
        // requires deleting one. `mcp` is the fifth and last.
        let verbs: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect();
        assert!(verbs.len() <= 5, "{verbs:?}");
        assert_eq!(verbs, vec!["index", "query", "schema", "mcp", "status"]);
    }
}
