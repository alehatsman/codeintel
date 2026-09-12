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
        } => index_cmd(&path, rebuild, langs),
        Command::Query {
            program,
            path,
            format,
            limit,
            no_refresh,
        } => query_cmd(&program, &path, format, limit, no_refresh),
    }
}

fn index_cmd(path: &std::path::Path, rebuild: bool, langs: Vec<String>) -> Result<ExitCode> {
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
    let report = index::refresh(
        &mut store,
        &Plan {
            rebuild,
            langs,
            deadline: None,
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
    // Tier B is M3. Until then, say so with the command that closes the gap
    // rather than leaving the user to find it.
    eprintln!("  no SCIP index ingested (tier B lands at M3): rust-analyzer scip .");
    Ok(ExitCode::SUCCESS)
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
