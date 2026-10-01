//! Corpus and robustness tests for the frontend (Commit 02 "tests/fuzzing").
//!
//! `cargo-fuzz` needs a nightly toolchain, which this machine does not
//! have, so these tests provide the stable-toolchain equivalent: the
//! lexer and parser must never panic, whatever bytes they are fed.

use lobster_lexer::lex;
use lobster_parser::parse;
use lobster_source::SourceManager;

fn lex_and_parse(text: &str) -> usize {
    let mut sm = SourceManager::new();
    let id = sm.add_file("t.lobster", text);
    let lexed = lex(&sm, id, text);
    let parsed = parse(id, &lexed.tokens);
    lexed.diagnostics.len() + parsed.diagnostics.len()
}

#[test]
fn hello_lobster_parses_clean() {
    let text = include_str!("../../../examples/hello.lobster");
    assert_eq!(lex_and_parse(text), 0);
}

#[test]
fn every_truncation_of_hello_terminates() {
    let text = include_str!("../../../examples/hello.lobster");
    // Byte-wise prefixes may split UTF-8; char-boundary prefixes only.
    let mut len = 0;
    for ch in text.chars() {
        len += ch.len_utf8();
        let _ = lex_and_parse(&text[..len]);
    }
}

#[test]
fn every_single_byte_terminates() {
    let mut buf = [0u8; 4];
    for b in 0u8..=255 {
        // Single byte, plus the byte doubled and wrapped in braces.
        let one = (b as char).to_string();
        let _ = lex_and_parse(&one);
        buf[0] = b;
        buf[1] = b;
        let two = String::from_utf8_lossy(&buf[..2]).into_owned();
        let _ = lex_and_parse(&two);
        let _ = lex_and_parse(&format!("fn f() {{ {one} }}"));
    }
}

#[test]
fn hostile_nesting_terminates_with_errors() {
    for text in [
        "{{{[[[(((",
        "fn fn fn fn",
        "let let let;",
        "\"\"\"\"\"\"\"\"",
        "''''''",
        "/* /* /*",
        "0x 0b 0o",
        "=> => =>",
        "fn f() { match x { ",
        "struct S { x: ",
        "import :::;",
        "@@@ ### $$$",
        "a + * b",
        "((((((((((",
        "))))))))))",
    ] {
        let n = lex_and_parse(text);
        assert!(n > 0, "expected errors for {text:?}");
    }
}
