//! `codeintel mcp` — one tool, over JSON-RPC on stdin and stdout.
//!
//! **One tool, named `code_query`.** dex's `internal/mcp` is 17,763 lines and
//! needed a lint to stop its tool descriptions arguing with each other
//! ([research.md](../../../docs/research.md) §1a). The failure is structural: N
//! tools means N descriptions competing for the router's attention, and each
//! new question is tempted to become tool N+1. One tool plus a query language
//! means a new question costs zero surface.
//!
//! **No MCP SDK.** The stdio transport is newline-delimited JSON-RPC 2.0 and
//! `serde_json` is already here for the response contract, so the protocol is
//! the ~150 lines below rather than a dependency tree. Recorded in
//! [research.md](../../../docs/research.md) §6 with that reasoning.

use std::io::{BufRead, Write};
use std::path::Path;

use anyhow::{Context, Result};

use crate::census::Census;
use crate::query::{self, Options};
use crate::schema::Schema;
use crate::status::Status;

/// The MCP revision this server speaks.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// The only tool. Growing to two requires deleting this one
/// (`specs/00-overview.md` § Surface budget).
pub const TOOL: &str = "code_query";

/// JSON-RPC's code for a method the server does not implement.
const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC's code for parameters that do not fit the method.
const INVALID_PARAMS: i64 = -32602;

/// Serve on `input`/`output` until the client closes the stream.
///
/// Every line is one JSON-RPC message. A request carries an `id` and gets a
/// response; a notification has none and gets silence, which is what makes
/// `notifications/initialized` a no-op rather than an error.
///
/// # Errors
/// A write to `output` that fails. A malformed *message* is answered with a
/// JSON-RPC error, not propagated — one bad request must not end the session.
pub fn serve(root: &Path, input: impl BufRead, mut output: impl Write) -> Result<()> {
    for line in input.lines() {
        let line = line.context("reading a request")?;
        if line.trim().is_empty() {
            continue;
        }
        let Some(response) = respond(root, &line) else {
            continue;
        };
        writeln!(output, "{response}").context("writing a response")?;
        output.flush().context("flushing a response")?;
    }
    Ok(())
}

/// One message in, at most one message out.
fn respond(root: &Path, line: &str) -> Option<serde_json::Value> {
    let request: serde_json::Value = match serde_json::from_str(line) {
        Ok(request) => request,
        // No id to answer with, so this cannot be a JSON-RPC response at all.
        Err(_) => return None,
    };
    // A notification has no `id` and takes no reply, by the spec.
    let id = request.get("id")?.clone();
    let method = request.get("method").and_then(serde_json::Value::as_str)?;
    let params = request
        .get("params")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    Some(match method {
        "initialize" => ok(&id, &initialize()),
        "tools/list" => ok(&id, &serde_json::json!({ "tools": [tool()] })),
        "tools/call" => match call(root, &params) {
            Ok(result) => ok(&id, &result),
            Err(message) => error(&id, INVALID_PARAMS, &message),
        },
        "ping" => ok(&id, &serde_json::json!({})),
        other => error(&id, METHOD_NOT_FOUND, &format!("no method `{other}`")),
    })
}

/// What this server is and what it can do.
fn initialize() -> serde_json::Value {
    serde_json::json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "codeintel",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

/// The tool declaration. `rule` and `args` are **not** here: positional binding
/// is how the deleted `rules` verb lied — integers and their string forms are
/// different atoms (`specs/01-facts.md` § Integers), so `args: ["142"]` bound
/// the string and returned `ok` with zero rows. An agent writing the Datalog
/// itself gets the integer right.
fn tool() -> serde_json::Value {
    serde_json::json!({
        "name": TOOL,
        "description": "Query the structural code graph in Datalog. Facts: definitions, \
                        spans, containment, imports, references, implements. Call with \
                        {\"schema\":true} first to get the relation catalog and examples.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Datalog program ending in a ?- goal.",
                },
                "schema": {
                    "type": "boolean",
                    "description": "Return the relation catalog and exit.",
                },
                "limit": { "type": "integer", "default": 200 },
                "format": {
                    "type": "string",
                    "enum": ["text", "json"],
                    "default": "text",
                },
                "raw": {
                    "type": "boolean",
                    "description": "Print SymIds instead of `Name path:line`. \
                                    Use when feeding one answer into the next query.",
                },
            },
        },
    })
}

/// Run one `code_query` call.
///
/// `Err` is for a call that is malformed as a *call* — no arguments, both modes
/// at once. Everything a query can do wrong is an `Answer` with a status and a
/// hint, because that is what the taxonomy is for.
fn call(root: &Path, params: &serde_json::Value) -> Result<serde_json::Value, String> {
    let arguments = params.get("arguments").unwrap_or(&serde_json::Value::Null);
    let program = arguments.get("query").and_then(serde_json::Value::as_str);
    let wants_schema = arguments
        .get("schema")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    match (program, wants_schema) {
        (Some(_), true) => {
            return Err("give `query` or `schema`, not both".to_string());
        }
        (None, false) => {
            return Err(
                "give `query` (a Datalog program ending in a `?-` goal) or `schema: true`"
                    .to_string(),
            );
        }
        _ => {}
    }

    let json = arguments.get("format").and_then(serde_json::Value::as_str) == Some("json");
    if wants_schema {
        return Ok(schema(root, json));
    }

    let options = Options {
        limit: arguments
            .get("limit")
            .and_then(serde_json::Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(200),
        // An agent editing files is the assumed case, not the exception, so the
        // refresh is not exposed as a flag here (`specs/05-surface.md`).
        no_refresh: false,
        raw: arguments
            .get("raw")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        // Not exposed: a rule file is a path on the server's disk, and an
        // agent choosing one over the wire is a capability this tool does not
        // need. `codeintel query --rules` is where that lives.
        rules: Vec::new(),
    };
    let answer = match query::run(root, program.unwrap_or_default(), &options) {
        Ok(answer) => answer,
        // An I/O failure the status taxonomy cannot express. Still a tool
        // result rather than a protocol error: the caller wants the text.
        Err(e) => return Ok(failed(&format!("codeintel: {e:#}"))),
    };

    let text = if json {
        query::to_json(&answer).to_string()
    } else {
        rendered(&answer, options.raw)
    };
    Ok(serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        // The full contract alongside the text, so a consumer that branches on
        // `status` never parses prose to find it.
        "structuredContent": query::to_json(&answer),
        "isError": !answer.status.answered(),
    }))
}

/// The catalog, which needs no query and works with no index.
fn schema(root: &Path, json: bool) -> serde_json::Value {
    let census = match facts::Store::open(root, &extract::fingerprint())
        .map_err(|e| e.to_string())
        .and_then(|store| Census::of(&store).map_err(|e| e.to_string()))
    {
        Ok(census) => census,
        Err(e) => return failed(&format!("codeintel: {e}")),
    };
    let schema = Schema { census: &census };
    let text = schema.to_string();
    serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": serde_json::json!({
            "status": Status::Ok.as_str(),
            "schema": if json { serde_json::Value::String(text.clone()) } else { serde_json::Value::Null },
        }),
        "isError": false,
    })
}

/// Rows as text, with the status and hint appended.
///
/// The status is never omitted: `ok` with zero rows and a missing index must be
/// distinguishable without reading prose (`specs/00-overview.md` invariant 6),
/// and here there is no stderr to put it on.
fn rendered(answer: &query::Answer, raw: bool) -> String {
    let mut out = String::new();
    for row in &answer.rows {
        out.push_str(&row.line(raw));
        out.push('\n');
    }
    out.push_str("status=");
    out.push_str(answer.status.as_str());
    out.push('\n');
    if let Some(hint) = &answer.hint {
        out.push_str("hint: ");
        out.push_str(hint);
        out.push('\n');
    }
    out
}

/// A tool result that is an error, carrying the text the caller needs.
fn failed(text: &str) -> serde_json::Value {
    serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "isError": true,
    })
}

fn ok(id: &serde_json::Value, result: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error(id: &serde_json::Value, code: i64, message: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}
