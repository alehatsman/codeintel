//! Entry point. Parses arguments, calls into the library, maps a status to an
//! exit code — and nothing else.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use codeintel::index::{self, Plan};
use codeintel::query::{self, Options};
use facts::{Lock, Store};

/// Query structural facts about a source tree with Datalog.
#[derive(Debug, Parser)]
#[command(name = "codeintel", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The verbs. The budget is five (`specs/00-overview.md` § Surface budget);
/// `schema`, `status` and `mcp` land at M4.
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
        Err(e) => {
            eprintln!("codeintel: {e:#}");
            ExitCode::FAILURE
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
        } => query_cmd(&program, &path, format, limit, no_refresh),
    }
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
    let mut generation = 0;
    if rebuild {
        generation = index::discard(&root)?;
    }

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
    limit: Option<usize>,
    no_refresh: bool,
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

    let options = Options {
        limit: limit.unwrap_or_else(|| Options::default().limit),
        no_refresh,
    };
    let answer = query::run(path, &source, &options)?;

    match format {
        Format::Json => println!("{}", query::to_json(&answer)),
        Format::Text => {
            for row in &answer.rows {
                println!("{row}");
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
    Ok(if answer.status.answered() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(2)
    })
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
        // requires deleting one. Three are still unbuilt (M4).
        let verbs: Vec<String> = Cli::command()
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect();
        assert!(verbs.len() <= 5, "{verbs:?}");
        assert_eq!(verbs, vec!["index", "query"]);
    }
}
