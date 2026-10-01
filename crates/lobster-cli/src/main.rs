//! `lobster` command-line interface.
//!
//! Commit 05 scope: real `check` (lex, parse, resolve, type-check) and real
//! `run` (lower HIR/MIR/SSA, verify SSA, execute the reference interpreter).
//! Every other subcommand is an honest stub: it exits with code 2 and says
//! so — it never pretends to succeed.

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
    /// Run a Lobster program with the reference interpreter.
    Run {
        /// Lobster source file to run.
        file: PathBuf,
        /// Print the lowered MIR CFG and exit without running.
        #[arg(long)]
        dump_mir: bool,
        /// Print the SSA CFG (after verification) and exit without running.
        #[arg(long)]
        dump_ssa: bool,
        /// Verify the SSA CFG, report `ssa ok`, and exit without running.
        #[arg(long)]
        verify_ssa: bool,
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
    // Name resolution and type checking (Commit 03).
    let (resolved, resolve_diags) = lobster_resolve::resolve(&parsed.file);
    let mut diags = resolve_diags;
    diags.extend(lobster_sema::check_file(&sources, &resolved, &parsed.file));
    for d in &diags {
        eprint!("{}", renderer.render(d));
    }
    if !diags.is_empty() {
        eprintln!("check failed: {} error(s) in {display}", diags.len());
        return ExitCode::from(1);
    }
    println!(
        "ok: {display} ({} items, {} lines)",
        parsed.file.items.len(),
        f.line_count()
    );
    ExitCode::SUCCESS
}

fn cmd_run(file: PathBuf, dump_mir: bool, dump_ssa: bool, verify_ssa: bool) -> ExitCode {
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
            Label::primary(span, "nothing to run here"),
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
        eprintln!("run failed: {n} error(s) in {display}");
        return ExitCode::from(1);
    }
    let (resolved, resolve_diags) = lobster_resolve::resolve(&parsed.file);
    let (program, tables, mut diags) = lobster_sema::check_program(&resolved, &parsed.file);
    let mut all = resolve_diags;
    all.append(&mut diags);
    for d in &all {
        eprint!("{}", renderer.render(d));
    }
    if !all.is_empty() {
        eprintln!("run failed: {} error(s) in {display}", all.len());
        return ExitCode::from(1);
    }
    let (hir, hir_diags) = lobster_hir::lower(&parsed.file, &resolved, &program, &tables);
    for d in &hir_diags {
        eprint!("{}", renderer.render(d));
    }
    if !hir_diags.is_empty() {
        eprintln!("run failed: {} error(s) in {display}", hir_diags.len());
        return ExitCode::from(1);
    }
    let mir = lobster_mir::lower(&hir);
    if dump_mir {
        print!("{}", lobster_mir::dump(&mir));
        return ExitCode::SUCCESS;
    }
    // SSA construction always runs: it must verify before execution so a
    // malformed CFG fails closed instead of executing.
    let ssa = lobster_ssa::build(&mir);
    if let Err(errors) = lobster_ssa::verify(&ssa) {
        for e in &errors {
            eprintln!("error[SSA-verify]: {e}");
        }
        eprintln!(
            "run failed: SSA verification ({} error(s)) in {display}",
            errors.len()
        );
        return ExitCode::from(1);
    }
    if dump_ssa {
        print!("{}", lobster_ssa::dump(&ssa));
        return ExitCode::SUCCESS;
    }
    if verify_ssa {
        println!("ssa ok: {display}");
        return ExitCode::SUCCESS;
    }
    match lobster_interp::run_main(&mir) {
        Ok(outcome) => {
            for line in &outcome.printed {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err(trap) => {
            if let Some(span) = trap.span {
                let d = Diagnostic::error(
                    trap.code.as_str(),
                    trap.message.clone(),
                    Label::primary(span, "trap here"),
                );
                eprint!("{}", renderer.render(&d));
            } else {
                eprintln!("error[{}]: {}", trap.code.as_str(), trap.message);
            }
            ExitCode::from(1)
        }
    }
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
        Command::Run {
            file,
            dump_mir,
            dump_ssa,
            verify_ssa,
        } => cmd_run(file, dump_mir, dump_ssa, verify_ssa),
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
