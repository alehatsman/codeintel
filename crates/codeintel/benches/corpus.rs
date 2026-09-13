//! Wall clock over the pinned corpora: `docs/plan.md` M6 § Pulled forward,
//! harness 2. Local only — CI's regression gate is `tests/counters.rs`.
//!
//! ```sh
//! provision apply --stream tasks/bench.yml                       # checks every corpus out first
//! cargo bench -p codeintel --bench corpus -- <name> <corpus-dir> # or one corpus directly
//! UPDATE_BASELINE=1 cargo bench -p codeintel --bench corpus -- <name> <corpus-dir>
//! ```
//!
//! `<name>` picks `benches/corpora/<name>/`, which holds `corpus.json`,
//! `queries.tsv` and `baseline.json`. The bench works on a copy of the corpus in
//! a temp directory, never on the checkout. It writes `target/bench/<name>.json`,
//! then compares it with that corpus's baseline: counters exactly, wall clock
//! at +20%. Wall clock is compared only on the host the baseline was measured
//! on, and nothing is compared against a different corpus commit.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use codeintel::index::{self, Plan};
use codeintel::query::{self, Options};
use facts::Store;
use serde_json::{Value, json};

const INDEX_RUNS: usize = 3;
const LOAD_RUNS: usize = 20;
const REINDEX_RUNS: usize = 10;
const QUERY_WARMUPS: usize = 3;
const QUERY_RUNS: usize = 30;
const CLI_RUNS: usize = 10;

/// A wall-clock metric this much over its baseline fails the run.
const TOLERANCE: f64 = 0.20;
/// A per-query p50 must also be this many milliseconds slower to fail. A
/// sub-millisecond query moves by 20% on scheduler noise alone.
const NOISE_FLOOR_MS: f64 = 1.0;

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(e) => {
            eprintln!("bench: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run() -> Result<bool> {
    // `cargo bench` passes `--bench` to a harness-less target; flags are not
    // ours.
    let args: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with("--"))
        .collect();
    let [name, corpus] = args.as_slice() else {
        bail!("usage: cargo bench -p codeintel --bench corpus -- <name> <corpus-dir>");
    };
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let spec = manifest.join("benches/corpora").join(name);
    let reindex_file = reindex_file(&spec)?;
    let set = query_set(&spec)?;
    let corpus = PathBuf::from(corpus)
        .canonicalize()
        .context("the corpus directory")?;
    let rev = git_head(&corpus);

    let tmp = tempfile::tempdir().context("tempdir")?;
    let root = tmp.path().canonicalize().context("tempdir")?;
    copy(&corpus, &root)?;
    println!(
        "corpus {} at {rev}, copied to {}",
        corpus.display(),
        root.display()
    );

    let (index_cold_ms, files, defs) = index_cold(&root)?;
    println!("index_cold_ms   {index_cold_ms:>9.1}   ({files} files, {defs} defs)");
    let load_warm_ms = load_warm(&root)?;
    println!("load_warm_ms    {load_warm_ms:>9.1}");

    let mut queries = Vec::new();
    let (mut pooled, mut pooled_cli) = (Vec::new(), Vec::new());
    for (id, goal) in &set {
        let measured = measure_query(&root, id, goal)?;
        println!(
            "query {id:<3} p50 {:>8.1}  p95 {:>8.1}  cli p50 {:>8.1}  {} rows  {} derived",
            measured.p50, measured.p95, measured.cli_p50, measured.rows, measured.derived
        );
        pooled.extend_from_slice(&measured.runs);
        pooled_cli.extend_from_slice(&measured.cli_runs);
        queries.push(measured.to_json(id, goal));
    }

    // One MCP session over the same set, warm after its first call.
    let mcp = measure_mcp(&root, &set)?;
    for (value, runs) in queries.iter_mut().zip(&mcp.per_query) {
        if let Some(object) = value.as_object_mut() {
            object.insert("mcp_p50_ms".to_string(), json!(percentile(runs, 0.50)));
        }
    }
    let mcp_pooled: Vec<f64> = mcp.per_query.iter().flatten().copied().collect();
    println!(
        "mcp             first {:.1}  p50 {:.1}  p95 {:.1}",
        mcp.first,
        percentile(&mcp_pooled, 0.50),
        percentile(&mcp_pooled, 0.95)
    );

    // Last: it edits the tree the queries above ran against.
    let reindex_one_ms = reindex_one(&root, &reindex_file)?;
    println!("reindex_one_ms  {reindex_one_ms:>9.1}");

    let current = json!({
        "corpus": { "name": name, "rev": rev, "files": files, "defs": defs },
        "host": host(),
        // Needs root to drop (`purge`, /proc/sys/vm/drop_caches). Not measured,
        // and saying so rather than leaving the key out.
        "cold_page_cache": null,
        // The session's first call loads the index. One sample, so reported
        // and never compared.
        "mcp_first_ms": mcp.first,
        "metrics": {
            "index_cold_ms": index_cold_ms,
            "load_warm_ms": load_warm_ms,
            "reindex_one_ms": reindex_one_ms,
            "query_p50_ms": percentile(&pooled, 0.50),
            "query_p95_ms": percentile(&pooled, 0.95),
            "cli_p50_ms": percentile(&pooled_cli, 0.50),
            "mcp_p50_ms": percentile(&mcp_pooled, 0.50),
            "mcp_p95_ms": percentile(&mcp_pooled, 0.95),
        },
        "queries": queries,
    });
    println!(
        "query pooled    p50 {:.1}  p95 {:.1}  cli p50 {:.1}",
        percentile(&pooled, 0.50),
        percentile(&pooled, 0.95),
        percentile(&pooled_cli, 0.50)
    );

    let out = manifest
        .join("../../target/bench")
        .join(format!("{name}.json"));
    write_json(&out, &current)?;
    println!("wrote {}", out.display());

    let baseline = spec.join("baseline.json");
    if std::env::var_os("UPDATE_BASELINE").is_some() {
        write_json(&baseline, &current)?;
        println!("wrote {}", baseline.display());
        return Ok(true);
    }
    let Ok(text) = std::fs::read_to_string(&baseline) else {
        println!(
            "no baseline at {}: nothing compared. UPDATE_BASELINE=1 writes one",
            baseline.display()
        );
        return Ok(false);
    };
    let base: Value = serde_json::from_str(&text).context("baseline.json")?;
    Ok(compare(&base, &current))
}

/// Median of `INDEX_RUNS` cold tier-A indexes, each into a fresh store.
fn index_cold(root: &Path) -> Result<(f64, usize, usize)> {
    let mut runs = Vec::new();
    let (mut files, mut defs) = (0, 0);
    for _ in 0..INDEX_RUNS {
        index::discard(root)?;
        let started = Instant::now();
        let mut store = Store::open(root, &extract::fingerprint())?;
        let report = index::refresh(&mut store, &Plan::default())?;
        runs.push(ms(started));
        (files, defs) = (report.indexed, report.counts.defs);
    }
    Ok((percentile(&runs, 0.50), files, defs))
}

/// p50 of opening the store and loading every segment, after one warm-up.
fn load_warm(root: &Path) -> Result<f64> {
    let fingerprint = extract::fingerprint();
    let mut runs = Vec::new();
    for i in 0..=LOAD_RUNS {
        let started = Instant::now();
        let store = Store::open(root, &fingerprint)?;
        let relations = store.load()?;
        let elapsed = ms(started);
        drop(relations);
        if i > 0 {
            runs.push(elapsed);
        }
    }
    Ok(percentile(&runs, 0.50))
}

/// p50 of re-indexing after `file` grows by a line. Each run must re-extract
/// exactly one file, or it measured something else.
fn reindex_one(root: &Path, file: &str) -> Result<f64> {
    let path = root.join(file);
    let mut runs = Vec::new();
    for i in 0..REINDEX_RUNS {
        let mut text = std::fs::read_to_string(&path).with_context(|| file.to_string())?;
        writeln!(text, "// bench edit {i}")?;
        std::fs::write(&path, text)?;
        let started = Instant::now();
        let mut store = Store::open(root, &extract::fingerprint())?;
        let report = index::refresh(&mut store, &Plan::default())?;
        runs.push(ms(started));
        if report.indexed != 1 {
            bail!(
                "reindex run {i} re-extracted {} files, not 1",
                report.indexed
            );
        }
    }
    Ok(percentile(&runs, 0.50))
}

struct Measured {
    runs: Vec<f64>,
    cli_runs: Vec<f64>,
    p50: f64,
    p95: f64,
    cli_p50: f64,
    status: String,
    rows: usize,
    derived: u64,
    demand: String,
}

impl Measured {
    fn to_json(&self, id: &str, goal: &str) -> Value {
        json!({
            "id": id, "query": goal,
            "status": self.status, "rows": self.rows,
            "derived": self.derived, "demand": self.demand,
            "p50_ms": self.p50, "p95_ms": self.p95, "cli_p50_ms": self.cli_p50,
        })
    }
}

/// One query through `query::run`, then through the release binary.
///
/// `query::run` opens and loads the store on every call, and so does
/// `codeintel mcp` today, so these numbers include the load. They are not the
/// warm-process numbers `plan.md` M6 asks for, because no warm process exists.
fn measure_query(root: &Path, id: &str, goal: &str) -> Result<Measured> {
    let options = Options {
        no_refresh: true,
        ..Options::default()
    };
    let mut runs = Vec::new();
    let mut first: Option<(String, usize, u64, String)> = None;
    for i in 0..QUERY_WARMUPS + QUERY_RUNS {
        let started = Instant::now();
        let answer = query::run(root, goal, &options)?;
        let elapsed = ms(started);
        let counters = (
            answer.status.as_str().to_string(),
            answer.rows.len(),
            answer.derived,
            answer.demand,
        );
        match &first {
            None => first = Some(counters),
            Some(seen) if *seen != counters => {
                bail!("query {id}: counters changed between runs: {seen:?} then {counters:?}")
            }
            Some(_) => {}
        }
        if i >= QUERY_WARMUPS {
            runs.push(elapsed);
        }
    }

    let mut cli_runs = Vec::new();
    for i in 0..=CLI_RUNS {
        let started = Instant::now();
        let out = Command::new(env!("CARGO_BIN_EXE_codeintel"))
            .args(["query", goal, "--no-refresh"])
            .current_dir(root)
            .output()
            .context("the binary runs")?;
        let elapsed = ms(started);
        if out.status.code() == Some(2) {
            bail!("query {id}: {}", String::from_utf8_lossy(&out.stderr));
        }
        if i > 0 {
            cli_runs.push(elapsed);
        }
    }

    let (status, rows, derived, demand) = first.context("at least one run")?;
    Ok(Measured {
        p50: percentile(&runs, 0.50),
        p95: percentile(&runs, 0.95),
        cli_p50: percentile(&cli_runs, 0.50),
        runs,
        cli_runs,
        status,
        rows,
        derived,
        demand,
    })
}

/// One `codeintel mcp` session's latencies.
struct Mcp {
    /// The first call, which loads the index.
    first: f64,
    /// Per query, the measured runs after warm-up.
    per_query: Vec<Vec<f64>>,
}

/// One `codeintel mcp` session answering every query in turn, called the way an
/// agent calls it: default limit, refresh on.
///
/// Latency is measured flush to flush. The server flushes after every
/// response and its input is already in memory, so each gap is one call.
fn measure_mcp(root: &Path, set: &[(String, String)]) -> Result<Mcp> {
    let per = QUERY_WARMUPS + QUERY_RUNS;
    let mut input = String::new();
    let mut calls = 0_usize;
    for (_, goal) in set {
        for _ in 0..per {
            calls += 1;
            let request = json!({
                "jsonrpc": "2.0", "id": calls, "method": "tools/call",
                "params": { "name": codeintel::mcp::TOOL, "arguments": { "query": goal } },
            });
            writeln!(input, "{request}")?;
        }
    }

    let mut stamps = Stamps::default();
    let started = Instant::now();
    codeintel::mcp::serve(root, input.as_bytes(), &mut stamps)?;
    if stamps.at.len() != calls {
        bail!("mcp: {} responses to {calls} calls", stamps.at.len());
    }
    let failed = String::from_utf8_lossy(&stamps.bytes)
        .lines()
        .filter(|line| line.contains("\"isError\":true"))
        .count();
    if failed > 0 {
        bail!("mcp: {failed} of {calls} calls answered with an error; that measures nothing");
    }

    let mut gaps = Vec::with_capacity(calls);
    let mut last = started;
    for &at in &stamps.at {
        gaps.push(gap(last, at));
        last = at;
    }
    Ok(Mcp {
        first: gaps.first().copied().unwrap_or(f64::NAN),
        per_query: gaps
            .chunks(per)
            .map(|chunk| chunk.iter().skip(QUERY_WARMUPS).copied().collect())
            .collect(),
    })
}

/// A writer that keeps what the server wrote and when it flushed.
#[derive(Default)]
struct Stamps {
    bytes: Vec<u8>,
    at: Vec<Instant>,
}

impl std::io::Write for Stamps {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.at.push(Instant::now());
        Ok(())
    }
}

/// Report every difference and return whether the run passes.
fn compare(base: &Value, current: &Value) -> bool {
    let (base_rev, rev) = (base.pointer("/corpus/rev"), current.pointer("/corpus/rev"));
    if base_rev != rev {
        println!(
            "REFUSED: the baseline measured corpus {base_rev:?}, this is {rev:?}. A baseline \
             against a different tree measures nothing"
        );
        return false;
    }
    let mut failed = Vec::new();

    // Counters compare exactly, on any host.
    let by_id = |v: &Value| -> Vec<(String, Value)> {
        v.get("queries")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|q| {
                (
                    q.get("id").map(Value::to_string).unwrap_or_default(),
                    q.clone(),
                )
            })
            .collect()
    };
    let (old, new) = (by_id(base), by_id(current));
    if old
        .iter()
        .map(|(id, _)| id)
        .ne(new.iter().map(|(id, _)| id))
    {
        failed.push("the query set differs from the baseline's".to_string());
    }
    for ((id, a), (_, b)) in old.iter().zip(&new) {
        for key in ["status", "rows", "derived", "demand"] {
            if a.get(key) != b.get(key) {
                failed.push(format!(
                    "query {id} {key}: {:?} -> {:?}",
                    a.get(key),
                    b.get(key)
                ));
            }
        }
    }

    if base.get("host") == current.get("host") {
        let metrics = base.get("metrics").and_then(Value::as_object);
        for (key, old) in metrics.into_iter().flatten() {
            let new = current
                .pointer(&format!("/metrics/{key}"))
                .and_then(Value::as_f64);
            let (Some(a), Some(b)) = (old.as_f64(), new) else {
                failed.push(format!("{key}: missing"));
                continue;
            };
            judge(key, a, b, 0.0, &mut failed);
        }
        for ((id, a), (_, b)) in old.iter().zip(&new) {
            let p50 = |q: &Value| q.get("p50_ms").and_then(Value::as_f64);
            if let (Some(a), Some(b)) = (p50(a), p50(b)) {
                judge(
                    &format!("query {id} p50_ms"),
                    a,
                    b,
                    NOISE_FLOOR_MS,
                    &mut failed,
                );
            }
        }
    } else {
        println!(
            "wall clock NOT compared: baseline host {:?}, this host {:?}. Counters were",
            base.get("host"),
            current.get("host")
        );
    }

    for f in &failed {
        println!("FAIL {f}");
    }
    failed.is_empty()
}

fn judge(name: &str, old: f64, new: f64, floor: f64, failed: &mut Vec<String>) {
    if new > old * (1.0 + TOLERANCE) && new - old > floor {
        failed.push(format!("{name}: {old:.1} -> {new:.1} ms"));
    } else if new < old * (1.0 - TOLERANCE) {
        println!("faster {name}: {old:.1} -> {new:.1} ms");
    }
}

/// The corpus's `queries.tsv`: `id<TAB>query`, `#` lines skipped. A missing or
/// empty set is an error naming the file, never an empty run.
fn query_set(spec: &Path) -> Result<Vec<(String, String)>> {
    let path = spec.join("queries.tsv");
    let text = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
    let set = text
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            l.split_once('\t')
                .map(|(id, goal)| (id.to_string(), goal.to_string()))
                .with_context(|| format!("{}: `{l}`", path.display()))
        })
        .collect::<Result<Vec<_>>>()?;
    if set.is_empty() {
        bail!("{}: no queries", path.display());
    }
    Ok(set)
}

/// The file `reindex_one_ms` edits, from the corpus's `corpus.json`.
fn reindex_file(spec: &Path) -> Result<String> {
    let path = spec.join("corpus.json");
    let text = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
    let value: Value = serde_json::from_str(&text).with_context(|| path.display().to_string())?;
    value
        .get("reindex_file")
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("{}: no `reindex_file`", path.display()))
}

/// Nearest rank. `values` need not be sorted.
fn percentile(values: &[f64], p: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "p is in 0..=1 and the product is at most the sample count"
    )]
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted
        .get(rank.saturating_sub(1))
        .copied()
        .unwrap_or(f64::NAN)
}

/// Milliseconds, rounded to the microsecond. Finer digits are noise, and in a
/// committed baseline they rewrite every line on every update.
fn ms(started: Instant) -> f64 {
    gap(started, Instant::now())
}

/// Milliseconds from `from` to `to`, rounded to the microsecond.
fn gap(from: Instant, to: Instant) -> f64 {
    (to.duration_since(from).as_secs_f64() * 1_000_000.0).round() / 1000.0
}

fn host() -> Value {
    json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "cpus": std::thread::available_parallelism().map_or(0, usize::from),
    })
}

fn git_head(dir: &Path) -> String {
    Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map_or_else(
            || "unknown".to_string(),
            |o| String::from_utf8_lossy(&o.stdout).trim().to_string(),
        )
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(value)?;
    std::fs::write(path, text + "\n").with_context(|| path.display().to_string())
}

/// The corpus minus its history, its build output and any index it carries.
fn copy(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" || name == "target" || name == ".codeintel" {
            continue;
        }
        let target = to.join(&name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
