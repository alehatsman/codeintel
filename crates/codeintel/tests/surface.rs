//! `codeintel schema` and `codeintel status`.
//!
//! These are `docs/plan.md` M4's done-when list for the two read-only verbs.
//! What they guard is a single failure mode: a catalogue that *declares* what
//! the index holds instead of *counting* it. A static list advertising sixteen
//! kinds when the index holds five is invariant 5 broken by the onboarding
//! text itself.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use codeintel::schema;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_codeintel")
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rust")
}

fn tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy(&fixture(), dir.path());
    for extra in ["expected.facts", "index.scip", ".gitignore"] {
        drop(std::fs::remove_file(dir.path().join(extra)));
    }
    dir
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("readable") {
        let entry = entry.expect("entry");
        let name = entry.file_name();
        if name == "target" || name == ".codeintel" {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().expect("file type").is_dir() {
            copy(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).expect("copy");
        }
    }
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(binary())
        .args(args)
        .current_dir(root)
        .output()
        .expect("the binary runs")
}

fn stdout(root: &Path, args: &[&str]) -> String {
    let out = run(root, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The `schema` size ceiling, in **characters**, because characters are what
/// can be measured here.
///
/// `specs/05-surface.md` states the budget in tokens (~1500), and no tokenizer
/// is available offline — adding one is a dependency for a number that gates
/// copy length. Estimates of this text span **~1,400 tokens** (4 chars/token,
/// prose-like) to **~1,750** (a BPE-shaped count that charges per punctuation
/// mark and per identifier fragment, which is what dense tabular output really
/// looks like). The honest statement is that the output sits *at* its budget,
/// not comfortably inside it: `specs/05-surface.md` § `schema` records the band.
///
/// This is now the *only* budget the standard library spends against — the rule
/// count cap was dropped (`specs/00-overview.md` § Surface budget), because a
/// rule's cost to an agent is the bytes it must read, and a count cap pushed
/// back against the one growth path the design actually wants.
///
/// So this asserts the thing it can: the text does not grow. A change that
/// pushes past it is a change that has to argue for itself.
const MAX_SCHEMA_CHARS: usize = 5_700;

#[test]
fn the_schema_fits_its_size_budget_with_the_rule_list_complete() {
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let text = stdout(dir.path(), &["schema"]);

    assert!(
        text.len() <= MAX_SCHEMA_CHARS,
        "schema is {} chars, ceiling is {MAX_SCHEMA_CHARS}. The rule list must \
         stay complete, so the copy is what gives — or the standard library is \
         too big and a rule goes.",
        text.len()
    );

    for rule in schema::rules(schema::STDLIB) {
        assert!(text.contains(&rule.head), "{} is not advertised", rule.head);
    }
}

#[test]
fn the_rule_list_matches_the_rules() {
    // The `%%` signature is written out, so it can drift from the clause below
    // it. Every advertised head must name a real predicate at its real arity,
    // and every predicate in the file must be advertised — otherwise `schema`
    // is a catalogue of a stdlib that no longer exists.
    let advertised: Vec<(String, usize)> = schema::rules(schema::STDLIB)
        .iter()
        .map(|r| {
            let (name, arity) =
                schema::split_head(&r.head).unwrap_or_else(|| panic!("`{}` is not a head", r.head));
            (name.to_string(), arity)
        })
        .collect();

    let mut defined: Vec<(String, usize)> = Vec::new();
    for line in schema::STDLIB.lines() {
        let head = line.split_once(":-").map_or(line, |(h, _)| h);
        let Some((name, args)) = head.split_once('(') else {
            continue;
        };
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            continue;
        }
        let Some((args, _)) = args.rsplit_once(')') else {
            continue;
        };
        let row = (name.to_string(), args.split(',').count());
        if !defined.contains(&row) {
            defined.push(row);
        }
    }

    for rule in &advertised {
        assert!(
            defined.contains(rule),
            "`{}/{}` is advertised but not defined at that arity",
            rule.0,
            rule.1
        );
    }
    for rule in &defined {
        assert!(
            advertised.contains(rule),
            "`{}/{}` is defined but has no `%%` line, so `schema` hides it",
            rule.0,
            rule.1
        );
    }
    assert_eq!(advertised.len(), 38, "the rule count moved: {advertised:?}");
}

#[test]
fn schema_counts_come_from_this_index() {
    // The whole point: a kind at 0 is a kind the agent must not query, and it
    // is only knowable by counting.
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let text = stdout(dir.path(), &["schema"]);

    // The fixture is Rust, so it has functions and no classes.
    assert!(text.contains("class 0"), "{text}");
    assert!(!text.contains("function 0"), "{text}");
    // Tier A only: every reference is provenance `name`.
    assert!(text.contains("exact 0"), "{text}");
    assert!(text.contains("Lang  rust"), "{text}");
}

#[test]
fn schema_with_no_index_says_so_rather_than_reading_as_an_inventory() {
    // Zero everywhere is indistinguishable from "your code has none of these"
    // unless the output says which it is.
    let dir = tempfile::tempdir().expect("tempdir");
    let text = stdout(dir.path(), &["schema"]);
    assert!(text.contains("THERE IS NO INDEX HERE"), "{text}");
    assert!(text.contains("codeintel index ."), "{text}");
    // The catalogue is still there: an agent can learn the system before
    // indexing anything.
    assert!(text.contains("innermost_at"), "{text}");
}

#[test]
fn status_reports_per_language_counts_and_the_fingerprint() {
    // "python: 1,204 files, 11 defs" is visibly absurd to a human in one
    // second; `status: ok` is not (`docs/plan.md` M4).
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let text = stdout(dir.path(), &["status"]);
    assert!(text.starts_with("status: ok"), "{text}");
    assert!(text.contains("extractor: blake3:"), "{text}");
    assert!(text.contains("rust "), "{text}");
    assert!(text.contains("defs,"), "{text}");
    // Provenance is never blended into one number.
    assert!(text.contains("exact refs"), "{text}");
    assert!(text.contains("name refs"), "{text}");
    assert!(text.contains("changed since index: none"), "{text}");
    assert!(text.contains("scip: none"), "{text}");
}

#[test]
fn status_json_is_the_bug_report_artifact() {
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let out = stdout(dir.path(), &["status", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("JSON");

    assert_eq!(json["status"], "ok");
    assert!(
        json["extractor_fingerprint"]
            .as_str()
            .is_some_and(|f| f.starts_with("blake3:")),
        "{json}"
    );
    assert!(json["languages"]["rust"]["defs"].as_u64().unwrap_or(0) > 0);
    assert!(json["languages"]["rust"]["refs_name"].as_u64().unwrap_or(0) > 0);
    assert_eq!(
        json["languages"]["rust"]["refs_exact"], 0,
        "tier A only here"
    );
    // Every relation, including the ones at zero — an absent key would read as
    // "not measured" rather than "empty".
    for rel in facts::RELATIONS {
        assert!(
            json["relations"][rel.name].is_number(),
            "{} is missing from status json",
            rel.name
        );
    }
    assert!(json["scip"].as_array().is_some_and(Vec::is_empty));
}

#[test]
fn status_notices_a_file_that_changed_after_the_index() {
    let dir = tree();
    run(dir.path(), &["index", "."]);
    std::fs::write(dir.path().join("src/store.rs"), "pub fn added() {}\n").expect("write");

    let text = stdout(dir.path(), &["status"]);
    assert!(text.starts_with("status: stale"), "{text}");
    assert!(text.contains("src/store.rs"), "{text}");
}

#[test]
fn status_with_no_index_is_an_answer_not_a_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run(dir.path(), &["status"]);
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("status: no-index"), "{text}");
    assert!(text.contains("run: codeintel index ."), "{text}");
}

#[test]
fn a_wrapped_rule_doc_is_joined_not_dropped() {
    // `about` is the one relation designed to answer everything in one round
    // trip, and its `Rel` vocabulary does not fit on one line. A parser that
    // demanded a signature on every `%%` line dropped the continuation, so an
    // agent learned five of the ten values and would never ask for the rest.
    let about = schema::rules(schema::STDLIB)
        .into_iter()
        .find(|r| r.head.starts_with("about("))
        .expect("about is advertised");
    for value in [
        "sig",
        "doc",
        "defined",
        "parent",
        "caller",
        "callee",
        "implements",
        "implementor",
        "test",
        "extern",
    ] {
        assert!(about.doc.contains(value), "`{value}` is not advertised");
    }
    assert!(
        !about.doc.ends_with('|'),
        "truncated at a wrap: {}",
        about.doc
    );

    let dir = tree();
    run(dir.path(), &["index", "."]);
    assert!(stdout(dir.path(), &["schema"]).contains("implementor"));
}

#[test]
fn every_relation_has_a_signature_of_the_right_arity() {
    // `signature()` is hand-written because argument *names* carry the
    // meaning, so it can drift from `facts::RELATIONS`. Adding a relation
    // without an entry would print a blank argument list in the project's
    // most-read output.
    for rel in facts::RELATIONS {
        let (args, _) = schema::signature(rel.name);
        assert!(!args.is_empty(), "{} has no signature", rel.name);
        let head = format!("{}{args}", rel.name);
        let (name, arity) = schema::split_head(&head)
            .unwrap_or_else(|| panic!("{} has a malformed signature {args}", rel.name));
        assert_eq!(name, rel.name);
        assert_eq!(arity, rel.arity, "{} signature is {args}", rel.name);
    }
}

#[test]
fn the_value_vocabularies_are_the_extractors_own() {
    // Re-declaring them here would let `schema` advertise a kind the extractor
    // cannot emit, or hide one it does.
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let text = stdout(dir.path(), &["schema"]);
    for kind in extract::lang::KINDS {
        assert!(text.contains(&format!(" {kind} ")), "{kind} is not listed");
    }
    for role in extract::lang::ROLES {
        assert!(text.contains(&format!(" {role} ")), "{role} is not listed");
    }
}

#[test]
fn status_with_no_index_does_not_walk_the_tree() {
    // The walk is a full gitignore-aware traversal, and with no index there is
    // nothing to report an unindexed extension against.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("notes.md"), "# hello\n").expect("write");
    let out = stdout(dir.path(), &["status", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("JSON");
    assert_eq!(json["status"], "no-index");
    assert!(
        json["unsupported"]
            .as_object()
            .is_some_and(serde_json::Map::is_empty),
        "{json}"
    );
}

/// One JSON-RPC exchange per line, in; the responses, out.
fn rpc(root: &Path, requests: &[serde_json::Value]) -> Vec<serde_json::Value> {
    use std::io::Write as _;
    use std::process::Stdio;

    let mut child = Command::new(binary())
        .args(["mcp"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("the server starts");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for request in requests {
            writeln!(stdin, "{request}").expect("write");
        }
    }
    let out = child.wait_with_output().expect("the server exits");
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| serde_json::from_str(l).expect("a JSON-RPC response"))
        .collect()
}

#[test]
fn the_surface_budget_holds() {
    // `specs/00-overview.md` § Surface budget, asserted in CI. Three of these
    // cannot grow; the fourth is the one that does, which is why asserting
    // only the other three would make the founding thesis unfalsifiable.
    //
    // The verb count is asserted in `main.rs`, where the clap type lives.
    let dir = tempfile::tempdir().expect("tempdir");
    let tools = rpc(
        dir.path(),
        &[serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
    );
    let tools = tools[0]["result"]["tools"].as_array().expect("tools");
    assert_eq!(tools.len(), 1, "the MCP tool budget is 1: {tools:?}");
    assert_eq!(tools[0]["name"], codeintel::mcp::TOOL);

    assert!(
        facts::RELATIONS.len() <= 16,
        "base relations: {}",
        facts::RELATIONS.len()
    );

    // There is deliberately no cap on the rule count; see
    // `specs/00-overview.md` § Surface budget. The budget rules are spent
    // against is `schema` output size, asserted by
    // `the_schema_fits_its_size_budget_with_the_rule_list_complete`, because a
    // rule's real cost is the bytes an agent reads before it can work. A count
    // cap pushed back against the one growth path the design wants — a new
    // question is meant to become a rule, not a verb or a Rust helper.
}

#[test]
fn the_mcp_tool_takes_a_program_not_positional_arguments() {
    // `rule` + `args` carried the defect that deleted the `rules` verb: an
    // integer and its string form are different atoms, so `args: ["142"]`
    // bound the string and returned `ok` with zero rows.
    let dir = tempfile::tempdir().expect("tempdir");
    let out = rpc(
        dir.path(),
        &[serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})],
    );
    let properties = out[0]["result"]["tools"][0]["inputSchema"]["properties"]
        .as_object()
        .expect("properties");
    assert!(!properties.contains_key("rule"), "{properties:?}");
    assert!(!properties.contains_key("args"), "{properties:?}");
    assert!(properties.contains_key("query"));
    assert!(properties.contains_key("schema"));
}

#[test]
fn a_notification_gets_no_reply_and_a_request_does() {
    // A JSON-RPC message with no `id` is a notification. Answering one is a
    // protocol violation, and `notifications/initialized` is the one every
    // client sends immediately after `initialize`.
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let out = rpc(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
            serde_json::json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"ping"}),
        ],
    );
    assert_eq!(out.len(), 2, "a notification was answered: {out:?}");
    assert_eq!(out[0]["id"], 1);
    assert_eq!(out[0]["result"]["serverInfo"]["name"], "codeintel");
    assert_eq!(out[1]["id"], 2);
}

#[test]
fn the_mcp_answer_carries_the_status_taxonomy() {
    // There is no stderr here to put the status on, and `ok` with zero rows
    // must stay distinguishable from every failure (invariant 6).
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let out = rpc(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"code_query",
                "arguments":{"query":"?- def(S, F, \"function\", N)."}}}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"code_query",
                "arguments":{"query":"?- file(F, \"python\")."}}}),
            serde_json::json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{
                "name":"code_query",
                "arguments":{"query":"?- def(S, F,"}}}),
            serde_json::json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
                "name":"code_query",
                "arguments":{"query":"?- nosuchrelation(X)."}}}),
        ],
    );

    let rows = &out[0]["result"];
    assert_eq!(rows["isError"], false);
    assert_eq!(rows["structuredContent"]["status"], "ok");
    let text = rows["content"][0]["text"].as_str().expect("text");
    assert!(text.contains("status=ok"), "{text}");

    // `ok` and empty, with a hint saying which literal matched nothing.
    let empty = &out[1]["result"];
    assert_eq!(empty["structuredContent"]["status"], "ok");
    assert!(
        empty["structuredContent"]["hint"]
            .as_str()
            .is_some_and(|h| h.contains("matched 0 rows")),
        "{empty}"
    );

    // A broken query is a tool result with a status, not a protocol error.
    let bad = &out[2]["result"];
    assert_eq!(bad["isError"], true);
    assert_eq!(bad["structuredContent"]["status"], "invalid-query");
    assert!(bad["structuredContent"]["hint"].is_string(), "{bad}");

    // A relation that does not exist is `invalid-query`, not `ok` with zero
    // rows. A typo and "this is not true of your code" are different answers,
    // and returning the second for the first is the failure invariant 6 names.
    let unknown = &out[3]["result"];
    assert_eq!(unknown["structuredContent"]["status"], "invalid-query");
    assert!(
        unknown["structuredContent"]["hint"]
            .as_str()
            .is_some_and(|h| h.contains("nosuchrelation")),
        "the offending name must appear: {unknown}"
    );
}

#[test]
fn the_mcp_schema_call_works_with_no_index() {
    // `{"schema":true}` is the first call a client makes, and it must answer
    // before anything has been indexed.
    let dir = tempfile::tempdir().expect("tempdir");
    let out = rpc(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
                "name":"code_query","arguments":{"schema":true}}}),
            serde_json::json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
                "name":"code_query","arguments":{"schema":true,"query":"?- file(F, L)."}}}),
        ],
    );
    let text = out[0]["result"]["content"][0]["text"]
        .as_str()
        .expect("text");
    assert!(text.contains("innermost_at"), "{text}");
    assert!(text.contains("THERE IS NO INDEX HERE"), "{text}");

    // Both modes at once names the two rather than silently picking one.
    assert!(out[1]["error"]["message"].is_string(), "{:?}", out[1]);
}

#[test]
fn a_request_with_no_method_is_answered_rather_than_ignored() {
    // A caller blocked forever on a silent id is the worst outcome this server
    // has. Anything carrying an `id` gets a reply, including one that cannot be
    // routed.
    let dir = tempfile::tempdir().expect("tempdir");
    let out = rpc(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":9}),
            serde_json::json!({"jsonrpc":"2.0","id":10,"method":"ping"}),
        ],
    );
    assert_eq!(out.len(), 2, "a request went unanswered: {out:?}");
    assert_eq!(out[0]["id"], 9);
    assert_eq!(out[0]["error"]["code"], -32600);
    assert_eq!(out[1]["id"], 10);
}

#[test]
fn an_mcp_error_result_still_carries_a_status() {
    // The promise is that a consumer never parses prose to find the status.
    // The error paths are exactly where it would otherwise have to.
    let dir = tempfile::tempdir().expect("tempdir");
    let out = rpc(
        dir.path(),
        &[
            serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
            "name":"code_query","arguments":{"query":"?- def(S, F, K, N)."}}}),
        ],
    );
    let result = &out[0]["result"];
    assert_eq!(result["structuredContent"]["status"], "no-index");
    assert!(result["structuredContent"]["hint"].is_string(), "{result}");
}

#[test]
fn schema_json_carries_the_counts_structurally() {
    // The whole argument for the text form is that a value at 0 is a value not
    // to query. A consumer that has to regex prose to learn that is in the same
    // position `query` puts one in when `display` is the only thing on offer.
    let dir = tree();
    run(dir.path(), &["index", "."]);
    let out = stdout(dir.path(), &["schema", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("JSON");

    assert!(
        json["text"]
            .as_str()
            .is_some_and(|t| t.contains("RELATIONS"))
    );
    assert_eq!(json["indexed"], true);

    // Every relation, with its argument names and its row count.
    let relations = json["relations"].as_array().expect("relations");
    assert_eq!(relations.len(), facts::RELATIONS.len());
    for rel in relations {
        assert!(rel["args"].as_str().is_some_and(|a| a.starts_with('(')));
        assert!(rel["rows"].is_number());
    }

    // Closed vocabularies, zeros included.
    for kind in extract::lang::KINDS {
        assert!(json["kinds"][*kind].is_number(), "{kind} missing");
    }
    for role in extract::lang::ROLES {
        assert!(json["roles"][*role].is_number(), "{role} missing");
    }
    // This fixture is Rust with no SCIP: functions yes, classes no.
    assert!(json["kinds"]["function"].as_u64().unwrap_or(0) > 0);
    assert_eq!(json["kinds"]["class"], 0);

    // Rules come through with their signatures, not just names.
    let rules = json["rules"].as_array().expect("rules");
    assert_eq!(rules.len(), schema::rules(schema::STDLIB).len());
    assert!(
        rules
            .iter()
            .all(|r| r["head"].as_str().is_some_and(|h| h.contains('(')))
    );
}
