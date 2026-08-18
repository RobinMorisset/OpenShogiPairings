//! The env-var documentation tracks what the server actually reads.
//!
//! Two prose lists claim to be complete: the module comment of `src/main.rs`
//! (what `docs/guides` and the dev tooling point people at) and the
//! configuration table in `deploy/README.md` (which
//! `docs/reference/architecture.md` calls "the one table"). Prose has no
//! compiler: a variable added, renamed or dropped in code leaves both wrong
//! with nothing failing. Both had in fact drifted when this test was written —
//! `OSP_BACKUP_DIR` was missing from the `main.rs` list and
//! `OSP_EXTRA_ORIGINS` from the README table — which is the argument for it.
//!
//! Same shape as `every_route_is_documented_in_the_api_doc`: extract the truth
//! from the code, extract the claim from the docs, diff, and refuse to pass on
//! an extractor that found implausibly little.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Every `OSP_*` variable production code reads: the string argument of any
/// `var("…")` / `var_os("…")` call under `src/`, with `#[cfg(test)]` tails cut
/// the way the route-table test cuts them.
fn vars_read_by_code() -> BTreeSet<String> {
    fn walk(dir: &Path, out: &mut BTreeSet<String>) {
        for entry in fs::read_dir(dir).expect("crates/server/src should be readable") {
            let path = entry
                .expect("crates/server/src entry should be readable")
                .path();
            if path.is_dir() {
                walk(&path, out);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let text = fs::read_to_string(&path).expect("source files should be UTF-8");
            let production = text.split("#[cfg(test)]").next().unwrap_or("");
            for needle in ["var(\"", "var_os(\""] {
                for chunk in production.split(needle).skip(1) {
                    let Some((name, _)) = chunk.split_once('"') else {
                        continue;
                    };
                    if name.starts_with("OSP_") {
                        out.insert(name.to_owned());
                    }
                }
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    // A silent extraction failure must not read as "nothing needs documenting".
    assert!(
        out.len() >= 6,
        "found only {} env-var reads in src/ — the extractor is broken, not the docs: {out:?}",
        out.len(),
    );
    out
}

/// Every maximal `OSP_[A-Z_]*` token in `text` — the loosest sensible read of
/// "this document mentions that variable".
fn osp_tokens(text: &str) -> BTreeSet<String> {
    let bytes = text.as_bytes();
    let mut out = BTreeSet::new();
    let mut from = 0;
    while let Some(pos) = text[from..].find("OSP_") {
        let start = from + pos;
        let mut end = start;
        while end < bytes.len() && (bytes[end].is_ascii_uppercase() || bytes[end] == b'_') {
            end += 1;
        }
        out.insert(text[start..end].trim_end_matches('_').to_owned());
        from = end;
    }
    out
}

fn deploy_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy")
}

/// Both complete lists really are complete: every variable the server reads
/// has a line in the `main.rs` module comment and a row in `deploy/README.md`.
#[test]
fn every_env_var_read_is_documented() {
    let read = vars_read_by_code();

    let main_rs = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/main.rs"))
        .expect("src/main.rs should exist");
    let module_comment: String = main_rs
        .lines()
        .take_while(|line| line.starts_with("//!"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !module_comment.is_empty(),
        "src/main.rs no longer starts with a //! module comment — move this check to \
         wherever the canonical env-var list went"
    );

    let readme =
        fs::read_to_string(deploy_dir().join("README.md")).expect("deploy/README.md should exist");

    let mut missing: Vec<String> = read
        .iter()
        .flat_map(|var| {
            [
                (!osp_tokens(&module_comment).contains(var))
                    .then(|| format!("    {var}   (src/main.rs module comment)")),
                (!osp_tokens(&readme).contains(var))
                    .then(|| format!("    {var}   (deploy/README.md)")),
            ]
        })
        .flatten()
        .collect();
    missing.sort();

    assert!(
        missing.is_empty(),
        "the server reads these variables but the docs never mention them:\n{}",
        missing.join("\n"),
    );
}

/// Nothing in `deploy/` names a variable the server no longer reads — the
/// direction a rename rots: the code and its docs move together, the deploy
/// files stay behind still setting the old name, and the server silently
/// ignores it.
#[test]
fn deploy_names_no_env_var_the_server_does_not_read() {
    let read = vars_read_by_code();

    let mut scanned = 0;
    let mut stale: Vec<String> = Vec::new();
    for entry in fs::read_dir(deploy_dir()).expect("deploy/ should be readable") {
        let path = entry.expect("deploy/ entry should be readable").path();
        if !path.is_file() {
            continue;
        }
        let text = fs::read_to_string(&path).expect("deploy files should be UTF-8");
        scanned += 1;
        for token in osp_tokens(&text) {
            if !read.contains(&token) {
                stale.push(format!(
                    "    {token}   ({})",
                    path.file_name()
                        .expect("a file has a name")
                        .to_string_lossy(),
                ));
            }
        }
    }
    assert!(
        scanned >= 4,
        "found only {scanned} files in deploy/ — the scan is broken, not the docs",
    );

    stale.sort();
    assert!(
        stale.is_empty(),
        "deploy/ sets or documents these variables, but nothing in crates/server/src \
         reads them:\n{}",
        stale.join("\n"),
    );
}
