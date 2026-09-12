//! The seam: `crates/datalog` knows nothing about code.
//!
//! Invariant 6 of `specs/00-overview.md`. The engine is generic over interned
//! tuples; the moment it can name a symbol, code-intel concepts leak into
//! evaluation and the engine stops being testable on its own. Guarded
//! mechanically from commit one, because the failure is a one-line manifest
//! edit that no reviewer would blink at.

/// Dependency table names a manifest can carry, including the `target.*` forms.
fn is_dependency_table(header: &str) -> bool {
    header == "dependencies"
        || header == "dev-dependencies"
        || header == "build-dependencies"
        || (header.starts_with("target.") && header.ends_with("dependencies"))
}

/// Every dependency name declared by a manifest, from both table and inline
/// forms (`[dependencies]` keys, and `foo = { .. }` under a dependency table).
fn declared_dependencies(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut table = String::new();

    for raw in manifest.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix('[') {
            let Some(header) = rest.strip_suffix(']') else {
                continue;
            };
            table = header
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_string();
            // `[dependencies.foo]` declares `foo`.
            if let Some((head, name)) = table.rsplit_once('.')
                && is_dependency_table(head)
            {
                names.push(name.trim_matches('"').to_string());
            }
            continue;
        }
        if !is_dependency_table(&table) || line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, _)) = line.split_once('=') {
            let key = key.trim().trim_matches('"');
            // `foo.workspace = true` declares `foo`, not `foo.workspace`.
            let name = key.split('.').next().unwrap_or(key);
            if !name.is_empty() {
                names.push(name.to_string());
            }
        }
    }
    names
}

#[test]
fn datalog_depends_on_no_code_intel_crate() {
    let manifest = include_str!("../Cargo.toml");
    let deps = declared_dependencies(manifest);

    for banned in ["facts", "extract", "codeintel"] {
        assert!(
            !deps.iter().any(|d| d == banned),
            "crates/datalog must not depend on `{banned}`: the engine knows nothing \
             about code (specs/00-overview.md invariant 6). Declared: {deps:?}"
        );
    }
}

#[test]
fn datalog_depends_on_nothing_at_all() {
    let manifest = include_str!("../Cargo.toml");
    let deps = declared_dependencies(manifest);

    assert!(
        deps.is_empty(),
        "crates/datalog is a zero-dependency crate (docs/plan.md M1). \
         `match/2` arrives as an injected host builtin, not as a dependency here. \
         Declared: {deps:?}"
    );
}

#[test]
fn dependency_parser_sees_the_forms_a_manifest_uses() {
    let manifest = "\
[package]
name = \"probe\"

[dependencies]
serde = { version = \"1\" }
facts.workspace = true

[dev-dependencies.extract]
path = \"../extract\"

[target.'cfg(unix)'.dependencies]
libc = \"0.2\"
";
    let deps = declared_dependencies(manifest);
    assert_eq!(deps, vec!["serde", "facts", "extract", "libc"]);
}
