//! Compile-fail suite: every `tests/compile-fail/*.lobster` must fail with
//! exactly the diagnostics in its `.stderr` snapshot.
//!
//! Display names are the file names (no temp paths), so snapshots are
//! deterministic. Regenerate with `LOBSTER_UPDATE=1 cargo test -p lobster-sema`,
//! review the diff, then commit.

use lobster_diagnostics::Renderer;
use lobster_lexer::lex;
use lobster_parser::parse;
use lobster_source::SourceManager;
use std::path::PathBuf;

#[test]
fn compile_fail_suite() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/compile-fail");
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("compile-fail dir exists")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "lobster"))
        .collect();
    cases.sort();
    assert!(
        cases.len() >= 14,
        "expected at least 14 compile-fail cases, found {}",
        cases.len()
    );
    for case in cases {
        let name = case
            .file_name()
            .expect("file name")
            .to_str()
            .expect("utf8 name")
            .to_string();
        let text = std::fs::read_to_string(&case).expect("read case");
        let mut sm = SourceManager::new();
        let id = sm.add_file(name.clone(), text.clone());
        let lexed = lex(&sm, id, &text);
        let parsed = parse(id, &lexed.tokens);
        let (resolved, resolve_diags) = lobster_resolve::resolve(&parsed.file);
        let check_diags = lobster_sema::check_file(&sm, &resolved, &parsed.file);
        let renderer = Renderer::new(&sm);
        let mut out = String::new();
        for d in lexed
            .diagnostics
            .iter()
            .chain(parsed.diagnostics.iter())
            .chain(resolve_diags.iter())
            .chain(check_diags.iter())
        {
            out.push_str(&renderer.render(d));
        }
        assert!(
            !out.is_empty(),
            "{name} is a fail-case but produced no errors"
        );
        let expected_path = case.with_extension("stderr");
        if std::env::var("LOBSTER_UPDATE").is_ok() {
            std::fs::write(&expected_path, &out).expect("write snapshot");
        } else {
            let want = std::fs::read_to_string(&expected_path)
                .unwrap_or_else(|_| panic!("missing snapshot for {name}; run with LOBSTER_UPDATE=1"));
            assert_eq!(out, want, "diagnostic mismatch for {name}");
        }
    }
}
