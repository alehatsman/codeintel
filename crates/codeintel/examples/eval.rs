//! Run one Datalog query against the agent-eval fixture and print the rows.
//!
//! The harness behind `docs/agent-eval.md`. It stands in for `codeintel query`
//! at M1, when there is a fact store but no CLI yet — and it is an example
//! rather than a verb, because the surface budget is five verbs and this is
//! not one of them.
//!
//! ```sh
//! cargo run -p codeintel --example eval -- '?- def(S, F, "function", N).'
//! ```

use std::process::ExitCode;

use codeintel::{Regexes, load_facts, render_row};
use datalog::{Engine, Limits, Strings};

const FACTS: &str = include_str!("../../../tests/fixtures/eval/facts.dl");
const STDLIB: &str = include_str!("../../../rules/stdlib.dl");

fn main() -> ExitCode {
    let Some(query) = std::env::args().nth(1) else {
        eprintln!("usage: eval '<datalog query>'");
        return ExitCode::from(2);
    };

    let mut engine = Engine::new(Box::new(Strings::new())).with_regexes(Box::new(Regexes::new()));
    if let Err(diagnostic) = load_facts(&mut engine, FACTS) {
        eprintln!("fixture: {diagnostic}");
        return ExitCode::from(3);
    }
    if let Err(diagnostic) = engine.load_rules(STDLIB) {
        eprintln!("stdlib: {diagnostic}");
        return ExitCode::from(3);
    }

    match engine.query(&query, &Limits::default()) {
        Ok(result) => {
            println!("columns\t{}", result.columns.join("\t"));
            // Raw rows: the eval's fact files are hand-written, so there is no
            // index to resolve a symbol's location against.
            let mut rows: Vec<String> =
                result.rows.iter().map(|r| render_row(&engine, r)).collect();
            rows.sort();
            for row in rows {
                println!("{row}");
            }
            println!("rows\t{}", result.rows.len());
            if result.truncated {
                println!("truncated\t{}", result.cap.unwrap_or("?"));
            }
            ExitCode::SUCCESS
        }
        Err(diagnostic) => {
            println!("status\t{}", diagnostic.status);
            println!("message\t{}", diagnostic.message);
            ExitCode::from(1)
        }
    }
}
