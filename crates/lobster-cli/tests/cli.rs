use assert_cmd::Command;
use predicates::prelude::PredicateBooleanExt;
use std::io::Write as _;

fn lobster() -> Command {
    Command::cargo_bin("lobster").expect("lobster binary builds")
}

#[test]
fn help_lists_subcommands() {
    lobster()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicates::str::contains("check").and(predicates::str::contains("run")));
}

#[test]
fn check_ok_on_nonempty_file() {
    let mut f = tempfile::NamedTempFile::with_suffix(".lobster").unwrap();
    writeln!(f, "fn main() {{}}").unwrap();
    lobster()
        .args(["check", &f.path().display().to_string()])
        .assert()
        .success()
        .stdout(predicates::str::contains("ok:"));
}

#[test]
fn check_hello_lobster_parses() {
    let hello = concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/hello.lobster");
    lobster()
        .args(["check", hello])
        .assert()
        .success()
        .stdout(predicates::str::contains("2 items"));
}

#[test]
fn check_broken_file_reports_syntax_error() {
    let mut f = tempfile::NamedTempFile::with_suffix(".lobster").unwrap();
    writeln!(f, "fn f() {{ let x = ; }}\nfn g() {{}}").unwrap();
    lobster()
        .args(["check", &f.path().display().to_string()])
        .assert()
        .failure()
        .stderr(predicates::str::contains("E110").or(predicates::str::contains("E111")));
}

#[test]
fn check_missing_file_is_error_lobster001() {
    lobster()
        .args(["check", "does-not-exist.lobster"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("LOBSTER-001"));
}

#[test]
fn check_empty_file_is_error_lobster002() {
    let f = tempfile::NamedTempFile::with_suffix(".lobster").unwrap();
    lobster()
        .args(["check", &f.path().display().to_string()])
        .assert()
        .failure()
        .stderr(predicates::str::contains("LOBSTER-002"));
}

#[test]
fn unimplemented_subcommands_exit_2() {
    lobster()
        .arg("run")
        .assert()
        .code(2)
        .stderr(predicates::str::contains("LOBSTER-000"));
}
