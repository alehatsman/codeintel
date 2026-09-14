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

use std::io::{BufRead, Read, Write};
use std::path::Path;

use anyhow::{Context, Result};

use crate::census::Census;
use crate::query::{Options, Warm};
use crate::schema::Schema;
use crate::status::Status;
use crate::wire::{self, Wire};

/// The MCP revision this server speaks.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// The only tool. Growing to two requires deleting this one
/// (`specs/00-overview.md` § Surface budget).
pub const TOOL: &str = "code_query";

/// JSON-RPC's code for a line that is not JSON at all.
const PARSE_ERROR: i64 = -32700;
/// JSON-RPC's code for a message that is not a well-formed request.
const INVALID_REQUEST: i64 = -32600;
/// JSON-RPC's code for a method the server does not implement.
const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC's code for parameters that do not fit the method.
const INVALID_PARAMS: i64 = -32602;
/// JSON-RPC's code for a failure no status expresses: an I/O error
/// (`specs/05-surface.md` § MCP).
const INTERNAL_ERROR: i64 = -32603;

/// The longest line read into memory. A program is a few hundred bytes; this
/// bounds what one line can cost, not what a query can say.
pub const MAX_LINE_BYTES: usize = 1 << 20;

/// Serve on `input`/`output` until the client closes the stream.
///
/// Every line is one JSON-RPC message. A request carries an `id` and gets a
/// response; a notification has none and gets silence, which is what makes
/// `notifications/initialized` a no-op rather than an error.
///
/// # Errors
/// A read or write that fails. A malformed *message* — not UTF-8, not JSON,
/// a batch, too long — is answered with a JSON-RPC error, not propagated: one
/// bad line must not end the session (`specs/05-surface.md` § MCP).
pub fn serve(root: &Path, mut input: impl BufRead, mut output: impl Write) -> Result<()> {
    // One loaded index for the life of the session, rebuilt when a refresh
    // moves the manifest (`specs/05-surface.md` § MCP).
    let mut warm = Warm::default();
    let mut line = Vec::new();
    loop {
        line.clear();
        let limit = u64::try_from(MAX_LINE_BYTES).unwrap_or(u64::MAX);
        let read = (&mut input)
            .take(limit)
            .read_until(b'\n', &mut line)
            .context("reading a request")?;
        if read == 0 {
            return Ok(());
        }
        let response = if line.last() == Some(&b'\n') || read < MAX_LINE_BYTES {
            respond(root, &mut warm, &line)
        } else {
            // The cap stopped the read mid-line. Skip the rest without holding
            // it: bounding the buffer is the point.
            input.skip_until(b'\n').context("skipping a long request")?;
            Some(error(
                &serde_json::Value::Null,
                INVALID_REQUEST,
                &format!("a request longer than {MAX_LINE_BYTES} bytes"),
            ))
        };
        let Some(response) = response else {
            continue;
        };
        writeln!(output, "{response}").context("writing a response")?;
        output.flush().context("flushing a response")?;
    }
}

/// One line in, at most one message out.
fn respond(root: &Path, warm: &mut Warm, line: &[u8]) -> Option<serde_json::Value> {
    // No id can be read out of these, so the error goes to `id: null` — the
    // client that sent them is still waiting for something.
    let Ok(line) = std::str::from_utf8(line) else {
        return Some(error(
            &serde_json::Value::Null,
            PARSE_ERROR,
            "a request that is not UTF-8",
        ));
    };
    if line.trim().is_empty() {
        return None;
    }
    let request: serde_json::Value = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(e) => {
            return Some(error(
                &serde_json::Value::Null,
                PARSE_ERROR,
                &format!("a request that is not JSON: {e}"),
            ));
        }
    };
    if !request.is_object() {
        let message = if request.is_array() {
            "batches are not supported; send one request per line"
        } else {
            "a request is a JSON object"
        };
        return Some(error(&serde_json::Value::Null, INVALID_REQUEST, message));
    }
    // A notification has no `id` and takes no reply, by the spec. Everything
    // that DOES carry an id gets an answer, including one we cannot route — a
    // caller blocked forever on a silent id is the worst outcome here.
    let id = request.get("id")?.clone();
    let Some(method) = request.get("method").and_then(serde_json::Value::as_str) else {
        return Some(error(
            &id,
            INVALID_REQUEST,
            "a request needs a string `method`",
        ));
    };
    let params = request
        .get("params")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    Some(match method {
        "initialize" => ok(&id, &initialize()),
        "tools/list" => ok(&id, &serde_json::json!({ "tools": [tool()] })),
        "tools/call" => match call(root, warm, &params) {
            Ok(result) => ok(&id, &result),
            Err((code, message)) => error(&id, code, &message),
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
            // The repository is this process's working directory and there is
            // no argument for it. A caller that passes one — `path` is the
            // obvious guess, because the CLI has `--path` — must be told, not
            // quietly answered about a different tree. `ARGUMENTS` enforces it;
            // this declares it to a client that validates.
            "additionalProperties": false,
        },
    })
}

/// Every argument `code_query` accepts.
///
/// Unknown arguments were ignored, and the one a caller reaches for first is
/// `path`: the CLI takes `--path`, the tool description does not say the
/// repository is fixed, and an agent generalises. An MCP server started in one
/// repository, asked for `path: "<another repository>"`, answered from its own
/// tree with `status: "ok"` — a well-formed, confident answer about the wrong
/// codebase, which is the one failure mode invariant 6 exists to prevent.
const ARGUMENTS: [&str; 5] = ["query", "schema", "limit", "format", "raw"];

/// Run one `code_query` call.
///
/// `Err` is for a call that is malformed as a *call* — no arguments, both modes
/// at once. Everything a query can do wrong is an `Answer` with a status and a
/// hint, because that is what the taxonomy is for.
fn call(
    root: &Path,
    warm: &mut Warm,
    params: &serde_json::Value,
) -> Result<serde_json::Value, (i64, String)> {
    // Any other name ran `code_query` anyway, which answers a question the
    // caller did not ask under a name that does not exist.
    match params.get("name").and_then(serde_json::Value::as_str) {
        Some(TOOL) => {}
        Some(other) => {
            return Err((
                INVALID_PARAMS,
                format!("no tool `{other}`; the one tool is `{TOOL}`"),
            ));
        }
        None => {
            return Err((
                INVALID_PARAMS,
                format!("a tools/call needs a `name`; the one tool is `{TOOL}`"),
            ));
        }
    }
    let arguments = params.get("arguments").unwrap_or(&serde_json::Value::Null);
    // An argument this tool does not have is a question the caller thinks it is
    // asking and is not. `path` is the one that matters — see `ARGUMENTS`.
    if let Some(object) = arguments.as_object()
        && let Some(unknown) = object.keys().find(|k| !ARGUMENTS.contains(&k.as_str()))
    {
        let extra = if unknown == "path" {
            ". the repository is this server's working directory and cannot be \
             chosen per call; start one server per repository"
        } else {
            ""
        };
        return Err((
            INVALID_PARAMS,
            format!(
                "no argument `{unknown}`; `{TOOL}` takes {}{extra}",
                ARGUMENTS.join(", ")
            ),
        ));
    }
    let program = arguments.get("query").and_then(serde_json::Value::as_str);
    let wants_schema = arguments
        .get("schema")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    match (program, wants_schema) {
        (Some(_), true) => {
            return Err((
                INVALID_PARAMS,
                "give `query` or `schema`, not both".to_string(),
            ));
        }
        (None, false) => {
            return Err((
                INVALID_PARAMS,
                "give `query` (a Datalog program ending in a `?-` goal) or `schema: true`"
                    .to_string(),
            ));
        }
        _ => {}
    }

    let json = arguments.get("format").and_then(serde_json::Value::as_str) == Some("json");
    if wants_schema {
        return schema(root, json);
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
        // `max_result_bytes` is measured on the result this call returns.
        wire: if json { Wire::McpJson } else { Wire::McpText },
    };
    let answer = match warm.run(root, program.unwrap_or_default(), &options) {
        Ok(answer) => answer,
        // An I/O failure the status taxonomy cannot express: a JSON-RPC error,
        // as the CLI exits 2. A tool result needs a status, and the only
        // candidate, `corrupt`, tells the agent to delete a healthy index.
        Err(e) => return Err((INTERNAL_ERROR, format!("codeintel: {e:#}"))),
    };
    Ok(wire::tool_result(&answer, json, options.raw))
}

/// The catalog, which needs no query and works with no index.
fn schema(root: &Path, json: bool) -> Result<serde_json::Value, (i64, String)> {
    let census = match facts::Store::open(root, &extract::fingerprint())
        .and_then(|store| Census::of(&store))
    {
        Ok(census) => census,
        Err(e) => {
            let text = format!("codeintel: {e}");
            return match facts::fault(&e) {
                Some(fault) => Ok(failed(Status::of_fault(fault), &text)),
                None => Err((INTERNAL_ERROR, text)),
            };
        }
    };
    let schema = Schema { census: &census };
    // `content` carries the catalog in the format asked for — what
    // `codeintel schema` prints in that format. `structuredContent` carries
    // the same catalog, always as JSON, so a client that only reads one field
    // still gets the whole thing (`specs/05-surface.md` § MCP).
    let structured = wire::schema_json(&schema, &census, root);
    let body = if json {
        structured.to_string()
    } else {
        schema.to_string()
    };
    // `no-index` with no index, and still not an error: the catalog was
    // delivered, and it is the call a session makes before indexing anything
    // (`specs/05-surface.md` § `schema`).
    Ok(serde_json::json!({
        "content": [{ "type": "text", "text": body }],
        "structuredContent": structured,
        "isError": false,
    }))
}

/// A tool result that is an error, carrying the text the caller needs.
///
/// `structuredContent` is present here too: the promise is that a consumer
/// never parses prose to find the status, and an error path is exactly where
/// it would otherwise have to.
fn failed(status: Status, text: &str) -> serde_json::Value {
    serde_json::json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": { "status": status.as_str(), "hint": text },
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
