//! What an answer looks like on each wire it is sent on, and what it costs.
//!
//! `max_result_bytes` bounds output in bytes because the consumer is a context
//! window (`specs/00-overview.md` invariant 9). The printed text is not what
//! every surface sends: the CLI's JSON carries both notations of every row, and
//! an MCP result escapes its body into a JSON string. So the cap is measured by
//! encoding the answer the way it is sent, **with the same functions that send
//! it** — a size estimate kept beside a separate encoder is a second copy of
//! the format, and the two drift (`specs/05-surface.md` § Response contract).

use crate::query::Answer;

/// Where an answer goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Wire {
    /// `codeintel query`: rows on stdout, the status on stderr.
    #[default]
    Text,
    /// `codeintel query --format json`: one JSON document on stdout.
    Json,
    /// An MCP `tools/call` result in the default text format.
    McpText,
    /// An MCP `tools/call` result with `format: "json"`.
    McpJson,
}

impl Wire {
    /// The bytes `answer` costs sent this way.
    ///
    /// For MCP this is the tool result object — everything except the
    /// JSON-RPC envelope, whose `id` is whatever the client chose to send.
    #[must_use]
    pub fn bytes(self, answer: &Answer, raw: bool) -> usize {
        match self {
            Self::Text => answer.rows.iter().map(|r| r.line(raw).len() + 1).sum(),
            Self::Json => json(answer).to_string().len() + 1,
            Self::McpText | Self::McpJson => tool_result(answer, self == Self::McpJson, raw)
                .to_string()
                .len(),
        }
    }
}

/// Everything about an answer except its rows: what a consumer branches on.
#[must_use]
pub fn envelope(answer: &Answer) -> serde_json::Value {
    object(answer, false)
}

/// The full JSON answer, in the shape `specs/05-surface.md` § Response contract
/// specifies: the envelope plus the rows in both notations.
#[must_use]
pub fn json(answer: &Answer) -> serde_json::Value {
    object(answer, true)
}

/// One object, keys in the contract's order whether or not the rows are in it.
/// With `serde_json`'s `preserve_order` that is the printed order; without it
/// the keys sort, exactly as the `json!` literal this replaced did.
fn object(answer: &Answer, with_rows: bool) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert("status".into(), answer.status.as_str().into());
    map.insert("columns".into(), serde_json::json!(answer.columns));
    if with_rows {
        // The raw atoms, always: a programmatic consumer must never have to
        // parse the pretty form back apart (`specs/05-surface.md` § Symbol
        // rendering). `display` is the same rows, in the same order, with
        // symbols expanded — so `rows[i]` and `display[i]` are one tuple in two
        // notations.
        //
        // Values are carried structurally rather than tab-joined and split back
        // apart: a doc comment containing a tab would otherwise arrive as more
        // values than there are columns.
        let raw: Vec<&Vec<String>> = answer.rows.iter().map(|r| &r.raw).collect();
        let display: Vec<&Vec<String>> = answer.rows.iter().map(|r| &r.display).collect();
        map.insert("rows".into(), serde_json::json!(raw));
        map.insert("display".into(), serde_json::json!(display));
    }
    map.insert("truncated".into(), answer.truncated.into());
    map.insert("cap".into(), serde_json::json!(answer.cap));
    map.insert("hint".into(), serde_json::json!(answer.hint));
    map.insert(
        "stats".into(),
        serde_json::json!({
            "derived": answer.derived,
            "elapsed_ms": answer.elapsed_ms,
            "refreshed": answer.refreshed,
            "transformed": answer.transformed,
            "demand": answer.demand,
            "depends": answer.depends,
            "shadowed": answer.shadowed,
        }),
    );
    serde_json::Value::Object(map)
}

/// Rows as text, with the status and hint appended.
///
/// The status is never omitted: `ok` with zero rows and a missing index must be
/// distinguishable without reading prose (`specs/00-overview.md` invariant 6),
/// and in an MCP result there is no stderr to put it on.
#[must_use]
pub fn text(answer: &Answer, raw: bool) -> String {
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

/// One `code_query` result.
///
/// The rows travel **once**, in `content`, in the notation asked for.
/// `structuredContent` is the envelope alone, so a consumer branches on
/// `status` without parsing prose, and the answer is not paid for twice.
/// `content` has to be the complete answer: the 2024-11-05 revision this
/// server speaks has no `structuredContent`, and a client of it reads only
/// `content`.
#[must_use]
pub fn tool_result(answer: &Answer, json_format: bool, raw: bool) -> serde_json::Value {
    let body = if json_format {
        json(answer).to_string()
    } else {
        text(answer, raw)
    };
    serde_json::json!({
        "content": [{ "type": "text", "text": body }],
        "structuredContent": envelope(answer),
        "isError": !answer.status.answered(),
    })
}
