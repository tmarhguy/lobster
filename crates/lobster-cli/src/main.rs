//! `lobster` command-line interface.
//!
//! Commit 01 scope: real `check` (loads a file through the source manager
//! and reports diagnostics), honest stubs for everything else. A subcommand
//! that is not implemented yet exits with code 2 and says so — it never
//! pretends to succeed.

use clap::{Parser, Subcommand};
use lobster_diagnostics::{Diagnostic, Label, Renderer};
use lobster_source::SourceManager;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Debug, Parser)]
#[command(
    name = "lobster",
    version,
    about = "Lobster — a statically typed systems language and multi-target toolchain"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Scaffold a new Lobster project.
    New {
        /// Directory to create.
        path: Option<PathBuf>,
    },
    /// Parse and type-check a file (full checking lands in Commit 03).
    Check {
        /// Lobster source file to check.
        file: PathBuf,
    },
    /// Build a Lobster program.
    Build {
        /// Lobster source file to build.
        file: Option<PathBuf>,
    },
    /// Run a Lobster program (interpreter lands in Commit 04).
    Run {
        /// Lobster source file to run.
        file: Option<PathBuf>,
    },
    /// Run tests.
    Test {},
    /// Run benchmarks.
    Bench {},
    /// Format source files.
    Fmt {
        /// Check formatting without writing.
        #[arg(long)]
        check: bool,
    },
    /// Render documentation.
    Doc {},
    /// Start an interactive session.
    Repl {},
    /// Disassemble an object file.
    Objdump {},
    /// Debug a program.
    Debug {},
    /// Profile a program.
    Profile {},
}

/// LOBSTER-001: unreadable / missing input file.
fn file_open_error(sources: &SourceManager, display: &str, detail: &str) -> String {
    // No span exists for a file that failed to load, so report plainly.
    let _ = sources;
    format!("error[LOBSTER-001]: cannot read file '{display}': {detail}")
}

fn cmd_check(file: PathBuf) -> ExitCode {
    let display = file.display().to_string();
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) => {
            let sources = SourceManager::new();
            eprintln!("{}", file_open_error(&sources, &display, &e.to_string()));
            return ExitCode::from(1);
        }
    };
    let mut sources = SourceManager::new();
    let id = sources.add_file(display.clone(), text);
    let f = sources.get(id).expect("just added");
    if f.text().is_empty() {
        let span = sources.span(id, 0, 0).expect("empty file span");
        let d = Diagnostic::error(
            "LOBSTER-002",
            "empty source file",
            Label::primary(span, "nothing to check here"),
        )
        .with_note("write a Lobster program starting from examples/hello.lobster");
        eprint!("{}", Renderer::new(&sources).render(&d));
        return ExitCode::from(1);
    }
    let text = f.text().to_string();
    let lexed = lobster_lexer::lex(&sources, id, &text);
    let parsed = lobster_parser::parse(id, &lexed.tokens);
    let renderer = Renderer::new(&sources);
    for d in lexed.diagnostics.iter().chain(parsed.diagnostics.iter()) {
        eprint!("{}", renderer.render(d));
    }
    if !lexed.diagnostics.is_empty() || !parsed.diagnostics.is_empty() {
        let n = lexed.diagnostics.len() + parsed.diagnostics.len();
        eprintln!("check failed: {n} error(s) in {display}");
        return ExitCode::from(1);
    }
    println!(
        "ok: {display} ({} items, {} lines)",
        parsed.file.items.len(),
        f.line_count()
    );
    ExitCode::SUCCESS
}

fn not_implemented(name: &str) -> ExitCode {
    eprintln!("error[LOBSTER-000]: '{name}' is not implemented yet (roadmap: docs/status.md)");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::New { .. } => not_implemented("lobster new"),
        Command::Check { file } => cmd_check(file),
        Command::Build { .. } => not_implemented("lobster build"),
        Command::Run { .. } => not_implemented("lobster run"),
        Command::Test {} => not_implemented("lobster test"),
        Command::Bench {} => not_implemented("lobster bench"),
        Command::Fmt { .. } => not_implemented("lobster fmt"),
        Command::Doc {} => not_implemented("lobster doc"),
        Command::Repl {} => not_implemented("lobster repl"),
        Command::Objdump {} => not_implemented("lobster objdump"),
        Command::Debug {} => not_implemented("lobster debug"),
        Command::Profile {} => not_implemented("lobster profile"),
    }
}
