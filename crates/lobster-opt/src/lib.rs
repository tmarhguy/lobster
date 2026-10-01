//! Lobster generic optimizer: SSA passes with `O0`/`O1` levels.
//!
//! Commit 06 scope: machine-independent SSA simplification. The passes only
//! remove or rewrite provably redundant work; every trapping operation
//! (`/`, `%`, `as char`, bounds checks, calls, prints) is preserved bit for
//! bit, including whether it traps. Optimized output is re-verified with
//! [`lobster_ssa::verify`]; differential execution (original vs optimized)
//! is covered by this crate's tests via [`lobster_ssa::destruct`] plus the
//! reference interpreter.
//!
//! Soundness notes (the optimizer relies on these; the verifier checks the
//! cheap parts):
//! - Input is constructed SSA: every non-arm use is dominated by its
//!   definition. Copy propagation therefore substitutes freely in
//!   statements and terminators — but never into phi arms (an arm is
//!   evaluated in the predecessor, where the replacement may not dominate),
//!   except for constants, which dominate everywhere.
//! - Dead-code removal only drops total operations: the operation itself
//!   cannot trap and none of its operands is `Undef` (reading `Undef`
//!   traps, so dropping such a read would drop a trap). Calls, prints,
//!   field updates, casts, divisions, remainders, and slice indexing are
//!   never removed.
//! - `O0` performs no passes (build + verify only). `O1` runs all passes
//!   to fixpoint. Each pass strictly shrinks the program, so the loop
//!   always terminates.

use lobster_ast::BinOp;
use lobster_mir::Const_;
use lobster_ssa::{SsaFunc, SsaOperand, SsaProgram, SsaRvalue, SsaStmt, SsaTerm, SsaValue};
use std::collections::{HashMap, HashSet};

/// Optimization level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptLevel {
    /// No passes: build + verify only.
    O0,
    /// All passes to fixpoint.
    O1,
}

impl OptLevel {
    /// Parse a CLI `--opt-level` value.
    #[must_use]
    pub const fn parse(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::O0),
            1 => Some(Self::O1),
            _ => None,
        }
    }

    /// Numeric level (for status lines).
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::O0 => 0,
            Self::O1 => 1,
        }
    }
}

/// What one [`optimize`] run did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OptStats {
    /// Fixpoint iterations (0 for `O0`).
    pub iterations: u32,
    /// Phis folded (all arms identical).
    pub trivial_phis_folded: u32,
    /// Dead phis removed.
    pub dead_phis_removed: u32,
    /// Copies propagated.
    pub copies_propagated: u32,
    /// Dead pure assignments removed.
    pub dead_assigns_removed: u32,
}

impl OptStats {
    /// One-line summary for `--verify-ssa` output.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} folded, {} dead phis, {} copies, {} dead assigns ({} iters)",
            self.trivial_phis_folded,
            self.dead_phis_removed,
            self.copies_propagated,
            self.dead_assigns_removed,
            self.iterations
        )
    }
}

/// Optimize an SSA program. The input is left untouched; the output must
/// still pass [`lobster_ssa::verify`] (callers verify; tests assert it).
#[must_use]
pub fn optimize(program: &SsaProgram, level: OptLevel) -> (SsaProgram, OptStats) {
    let mut out = program.clone();
    let mut stats = OptStats::default();
    if level == OptLevel::O0 {
        return (out, stats);
    }
    for _ in 0..10_000 {
        stats.iterations += 1;
        let mut changed = false;
        changed |= pass_copy_prop(&mut out, &mut stats);
        changed |= pass_trivial_phi(&mut out, &mut stats);
        changed |= pass_dead_phi(&mut out, &mut stats);
        changed |= pass_dead_pure(&mut out, &mut stats);
        if !changed {
            break;
        }
    }
    (out, stats)
}

/// Count `Value` uses of every value (arms, rvalues, calls, prints,
/// terminators).
fn use_counts(func: &SsaFunc) -> HashMap<u32, usize> {
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for block in &func.blocks {
        for phi in &block.phis {
            for (_, op) in &phi.arms {
                bump_operand(&mut counts, op);
            }
        }
        for stmt in &block.stmts {
            match stmt {
                SsaStmt::Assign { rv, .. } => {
                    for o in rvalue_operands(rv) {
                        bump_operand(&mut counts, o);
                    }
                }
                SsaStmt::Call { target, args, .. } => {
                    if let lobster_ssa::SsaCallTarget::Value(o) = target {
                        bump_operand(&mut counts, o);
                    }
                    for a in args {
                        bump_operand(&mut counts, a);
                    }
                }
                SsaStmt::Print { values, .. } => {
                    for v in values {
                        bump_operand(&mut counts, v);
                    }
                }
                SsaStmt::SetField { base, value, .. } => {
                    bump_operand(&mut counts, base);
                    bump_operand(&mut counts, value);
                }
            }
        }
        match &block.term {
            SsaTerm::Branch { cond, .. } => bump_operand(&mut counts, cond),
            SsaTerm::Return(Some(o)) => bump_operand(&mut counts, o),
            _ => {}
        }
    }
    counts
}

fn bump_operand(counts: &mut HashMap<u32, usize>, op: &SsaOperand) {
    if let SsaOperand::Value(v) = op {
        *counts.entry(v.0).or_default() += 1;
    }
}

fn rvalue_operands(rv: &SsaRvalue) -> Vec<&SsaOperand> {
    match rv {
        SsaRvalue::Use(o) => vec![o],
        SsaRvalue::Binary { l, r, .. } => vec![l, r],
        SsaRvalue::Unary { v, .. } => vec![v],
        SsaRvalue::Cast { v, .. } => vec![v],
        SsaRvalue::Tuple(es) => es.iter().collect(),
        SsaRvalue::Field { base, .. } => vec![base],
        SsaRvalue::Enum { payload, .. } => payload.iter().collect(),
        SsaRvalue::Discriminant(b) | SsaRvalue::SliceLen(b) => vec![b],
        SsaRvalue::VariantPayload { base, .. } => vec![base],
        SsaRvalue::SliceIndex { base, idx } => vec![base, idx],
    }
}

fn operand_eq(a: &SsaOperand, b: &SsaOperand) -> bool {
    match (a, b) {
        (SsaOperand::Value(x), SsaOperand::Value(y)) => x == y,
        (SsaOperand::Const(x), SsaOperand::Const(y)) => const_eq(x, y),
        (SsaOperand::Undef, SsaOperand::Undef) => true,
        _ => false,
    }
}

fn const_eq(a: &Const_, b: &Const_) -> bool {
    match (a, b) {
        (Const_::Int(x, t), Const_::Int(y, u)) => x == y && t == u,
        (Const_::Float(x, t), Const_::Float(y, u)) => x == y && t == u,
        (Const_::Bool(x), Const_::Bool(y)) => x == y,
        (Const_::Char(x), Const_::Char(y)) => x == y,
        (Const_::Str(x), Const_::Str(y)) => x == y,
        (Const_::FuncRef(x), Const_::FuncRef(y)) => x == y,
        (Const_::Unit, Const_::Unit) => true,
        _ => false,
    }
}

fn is_const(op: &SsaOperand) -> bool {
    matches!(op, SsaOperand::Const(_))
}

/// Substitute `from` with `to` in every use position. Non-constant
/// replacements skip phi arms (an arm is evaluated in the predecessor,
/// where the replacement may not dominate); constants dominate everywhere.
fn replace_uses(func: &mut SsaFunc, from: SsaValue, to: &SsaOperand) {
    let everywhere = is_const(to);
    for block in &mut func.blocks {
        if everywhere {
            for phi in &mut block.phis {
                for (_, op) in &mut phi.arms {
                    swap_operand(op, from, to);
                }
            }
        }
        for stmt in &mut block.stmts {
            match stmt {
                SsaStmt::Assign { rv, .. } => swap_rvalue(rv, from, to),
                SsaStmt::Call { target, args, .. } => {
                    if let lobster_ssa::SsaCallTarget::Value(o) = target {
                        swap_operand(o, from, to);
                    }
                    for a in args {
                        swap_operand(a, from, to);
                    }
                }
                SsaStmt::Print { values, .. } => {
                    for v in values {
                        swap_operand(v, from, to);
                    }
                }
                SsaStmt::SetField { base, value, .. } => {
                    swap_operand(base, from, to);
                    swap_operand(value, from, to);
                }
            }
        }
        match &mut block.term {
            SsaTerm::Branch { cond, .. } => swap_operand(cond, from, to),
            SsaTerm::Return(Some(o)) => swap_operand(o, from, to),
            _ => {}
        }
    }
}

fn swap_operand(op: &mut SsaOperand, from: SsaValue, to: &SsaOperand) {
    if matches!(op, SsaOperand::Value(v) if *v == from) {
        *op = to.clone();
    }
}

fn swap_rvalue(rv: &mut SsaRvalue, from: SsaValue, to: &SsaOperand) {
    match rv {
        SsaRvalue::Use(o) => swap_operand(o, from, to),
        SsaRvalue::Binary { l, r, .. } => {
            swap_operand(l, from, to);
            swap_operand(r, from, to);
        }
        SsaRvalue::Unary { v, .. } => swap_operand(v, from, to),
        SsaRvalue::Cast { v, .. } => swap_operand(v, from, to),
        SsaRvalue::Tuple(es) => {
            for e in es {
                swap_operand(e, from, to);
            }
        }
        SsaRvalue::Field { base, .. } => swap_operand(base, from, to),
        SsaRvalue::Enum { payload, .. } => {
            for e in payload {
                swap_operand(e, from, to);
            }
        }
        SsaRvalue::Discriminant(b) | SsaRvalue::SliceLen(b) => {
            swap_operand(b, from, to);
        }
        SsaRvalue::VariantPayload { base, .. } => swap_operand(base, from, to),
        SsaRvalue::SliceIndex { base, idx } => {
            swap_operand(base, from, to);
            swap_operand(idx, from, to);
        }
    }
}

/// Follow an applied-substitution map to a fixpoint (cycle-safe).
fn resolve_through(op: &SsaOperand, applied: &HashMap<SsaValue, SsaOperand>) -> SsaOperand {
    let mut current = op.clone();
    let mut seen = HashSet::new();
    loop {
        match current {
            SsaOperand::Value(v) => match applied.get(&v) {
                Some(next) if seen.insert(v.0) => current = next.clone(),
                _ => return current,
            },
            _ => return current,
        }
    }
}

/// Fold phis whose arms are all identical into the shared value.
fn pass_trivial_phi(program: &mut SsaProgram, stats: &mut OptStats) -> bool {
    let mut changed = false;
    for func in program.funcs.values_mut() {
        let mut folds: Vec<(SsaValue, SsaOperand)> = Vec::new();
        for block in &func.blocks {
            for phi in &block.phis {
                if phi.arms.is_empty() {
                    continue;
                }
                let first = &phi.arms[0].1;
                if phi.arms.iter().all(|(_, op)| operand_eq(op, first)) {
                    folds.push((phi.dst, first.clone()));
                }
            }
        }
        // Chain-safe in any order: resolve each replacement through folds
        // already applied this pass.
        let mut applied: HashMap<SsaValue, SsaOperand> = HashMap::new();
        for (dst, to) in folds {
            let resolved = resolve_through(&to, &applied);
            replace_uses(func, dst, &resolved);
            applied.insert(dst, resolved);
            for block in &mut func.blocks {
                let before = block.phis.len();
                block.phis.retain(|p| p.dst != dst);
                if block.phis.len() != before {
                    changed = true;
                    stats.trivial_phis_folded += 1;
                }
            }
        }
    }
    changed
}

/// Remove phis nobody uses (phis never trap, so removal is always safe).
fn pass_dead_phi(program: &mut SsaProgram, stats: &mut OptStats) -> bool {
    let mut changed = false;
    for func in program.funcs.values_mut() {
        let counts = use_counts(func);
        for block in &mut func.blocks {
            let before = block.phis.len();
            block
                .phis
                .retain(|p| counts.get(&p.dst.0).copied().unwrap_or(0) > 0);
            let removed = before - block.phis.len();
            if removed > 0 {
                changed = true;
                stats.dead_phis_removed += u32::try_from(removed).unwrap_or(u32::MAX);
            }
        }
    }
    changed
}

/// Propagate `dst = use op` copies, then drop the copy.
/// - `Undef` copies are left alone (dropping them would drop a read that
///   traps).
/// - Non-constant copies whose `dst` appears in a phi arm are left alone:
///   the copy is removed, but arms are not rewritten (an arm is evaluated
///   in the predecessor, where the replacement may not dominate), so the
///   arm would dangle.
fn pass_copy_prop(program: &mut SsaProgram, stats: &mut OptStats) -> bool {
    let mut changed = false;
    for func in program.funcs.values_mut() {
        let mut arm_uses: HashSet<u32> = HashSet::new();
        for block in &func.blocks {
            for phi in &block.phis {
                for (_, op) in &phi.arms {
                    if let SsaOperand::Value(v) = op {
                        arm_uses.insert(v.0);
                    }
                }
            }
        }
        // Collect with positions; removal happens only for applied copies.
        let mut copies: Vec<(usize, usize, SsaValue, SsaOperand)> = Vec::new();
        for (bi, block) in func.blocks.iter().enumerate() {
            for (si, stmt) in block.stmts.iter().enumerate() {
                if let SsaStmt::Assign {
                    dst,
                    rv: SsaRvalue::Use(op),
                    ..
                } = stmt
                {
                    if matches!(op, SsaOperand::Undef) {
                        continue;
                    }
                    if !is_const(op) && arm_uses.contains(&dst.0) {
                        continue;
                    }
                    copies.push((bi, si, *dst, op.clone()));
                }
            }
        }
        // Chain-safe in any order: resolve through copies already applied.
        let mut applied: HashMap<SsaValue, SsaOperand> = HashMap::new();
        let mut remove: Vec<(usize, usize)> = Vec::new();
        for (bi, si, dst, op) in copies {
            let resolved = resolve_through(&op, &applied);
            replace_uses(func, dst, &resolved);
            applied.insert(dst, resolved);
            remove.push((bi, si));
        }
        remove.sort_unstable_by(|a, b| b.cmp(a));
        for (bi, si) in remove {
            func.blocks[bi].stmts.remove(si);
            changed = true;
            stats.copies_propagated += 1;
        }
    }
    changed
}

/// A rvalue that cannot trap and has no `Undef` operands: dropping its
/// (unused) definition is unobservable.
fn pure_removable(rv: &SsaRvalue) -> bool {
    let operands = rvalue_operands(rv);
    if operands.iter().any(|o| matches!(o, SsaOperand::Undef)) {
        return false;
    }
    match rv {
        SsaRvalue::Use(_)
        | SsaRvalue::Tuple(_)
        | SsaRvalue::Enum { .. }
        | SsaRvalue::Unary { .. } => true,
        SsaRvalue::Binary { op, .. } => !matches!(op, BinOp::Div | BinOp::Rem),
        SsaRvalue::Field { .. }
        | SsaRvalue::Discriminant(_)
        | SsaRvalue::VariantPayload { .. }
        | SsaRvalue::SliceLen(_) => true,
        // Casts (`u32 as char` traps), slice indexing (bounds trap):
        // never removed.
        SsaRvalue::Cast { .. } | SsaRvalue::SliceIndex { .. } => false,
    }
}

/// Drop unused pure assignments. Calls, prints, and field updates always
/// stay (side effects); trapping rvalues always stay (observable traps).
fn pass_dead_pure(program: &mut SsaProgram, stats: &mut OptStats) -> bool {
    let mut changed = false;
    for func in program.funcs.values_mut() {
        let counts = use_counts(func);
        let unused = |dst: &SsaValue| counts.get(&dst.0).copied().unwrap_or(0) == 0;
        for block in &mut func.blocks {
            let mut kept = Vec::with_capacity(block.stmts.len());
            for stmt in std::mem::take(&mut block.stmts) {
                match &stmt {
                    SsaStmt::Assign { dst, rv, .. } if unused(dst) && pure_removable(rv) => {
                        changed = true;
                        stats.dead_assigns_removed += 1;
                    }
                    _ => kept.push(stmt),
                }
            }
            block.stmts = kept;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use lobster_ssa::SsaProgram;

    fn ssa_of(text: &str) -> SsaProgram {
        let mut sm = lobster_source::SourceManager::new();
        let id = sm.add_file("t.lobster", text);
        let lexed = lobster_lexer::lex(&sm, id, text);
        assert!(lexed.diagnostics.is_empty());
        let parsed = lobster_parser::parse(id, &lexed.tokens);
        assert!(parsed.diagnostics.is_empty());
        let (resolved, rdiags) = lobster_resolve::resolve(&parsed.file);
        assert!(rdiags.is_empty(), "{rdiags:?}");
        let (program, tables, cdiags) = lobster_sema::check_program(&resolved, &parsed.file);
        assert!(cdiags.is_empty(), "{cdiags:?}");
        let (hir, _) = lobster_hir::lower(&parsed.file, &resolved, &program, &tables);
        lobster_ssa::build(&lobster_mir::lower(&hir))
    }

    fn phi_count(prog: &SsaProgram) -> usize {
        prog.funcs
            .values()
            .map(|f| f.blocks.iter().map(|b| b.phis.len()).sum::<usize>())
            .sum()
    }

    fn stmt_count(prog: &SsaProgram) -> usize {
        prog.funcs
            .values()
            .map(|f| f.blocks.iter().map(|b| b.stmts.len()).sum::<usize>())
            .sum()
    }

    #[test]
    fn o0_changes_nothing() {
        let ssa =
            ssa_of("fn main() {\n    let mut x = 1u32;\n    x += 2u32;\n    println(x);\n}\n");
        let (opted, stats) = optimize(&ssa, OptLevel::O0);
        assert_eq!(stats, OptStats::default());
        assert_eq!(lobster_ssa::dump(&opted), lobster_ssa::dump(&ssa));
        lobster_ssa::verify(&opted).expect("o0 verifies");
    }

    #[test]
    fn o1_shrinks_loop_phis_and_verifies() {
        let ssa = ssa_of(
            "fn f(n: u32) -> u32 {\n    let mut i = 0u32;\n    while i < n {\n        i += 1;\n    }\n    i\n}\nfn main() {\n    println(f(10u32));\n}\n",
        );
        lobster_ssa::verify(&ssa).expect("input verifies");
        let before = phi_count(&ssa);
        assert!(before > 0, "loop has join phis");
        let (opted, stats) = optimize(&ssa, OptLevel::O1);
        lobster_ssa::verify(&opted).expect("o1 verifies");
        assert!(phi_count(&opted) < before, "dead phis pruned: {stats:?}");
        assert!(stmt_count(&opted) <= stmt_count(&ssa));
    }

    #[test]
    fn o1_folds_identical_if_arms() {
        let ssa = ssa_of(
            "fn f(c: bool) -> u32 {\n    let x = if c {\n        1u32\n    } else {\n        2u32\n    };\n    x\n}\nfn main() {\n    println(f(true));\n}\n",
        );
        let (opted, _) = optimize(&ssa, OptLevel::O1);
        lobster_ssa::verify(&opted).expect("o1 verifies");
        // Copies collapse to constants, so the join needs fewer phis.
        assert!(phi_count(&opted) <= phi_count(&ssa));
    }

    #[test]
    fn levels_parse() {
        assert_eq!(OptLevel::parse(0), Some(OptLevel::O0));
        assert_eq!(OptLevel::parse(1), Some(OptLevel::O1));
        assert_eq!(OptLevel::parse(9), None);
        assert_eq!(OptLevel::O1.as_u8(), 1);
    }

    #[test]
    fn all_examples_optimize_and_verify() {
        for file in [
            "examples/hello.lobster",
            "examples/arithmetic.lobster",
            "examples/control_flow.lobster",
            "examples/structs_enums.lobster",
            "examples/strings.lobster",
            "examples/tomato_shapes.lobster",
        ] {
            let path = format!("../../{file}");
            let text = std::fs::read_to_string(&path).unwrap();
            let ssa = ssa_of(&text);
            let (opted, _) = optimize(&ssa, OptLevel::O1);
            lobster_ssa::verify(&opted).unwrap_or_else(|e| panic!("{file}: {e:?}"));
        }
    }

    // --- differential execution: original vs optimized must behave the same.

    fn run_both(text: &str, func: &str, args: Vec<lobster_interp::Value>) {
        let ssa = ssa_of(text);
        let (opted, _) = optimize(&ssa, OptLevel::O1);
        lobster_ssa::verify(&opted).expect("optimized verifies");
        let before = lobster_interp::run(&lobster_ssa::destruct(&ssa), func, args.clone());
        let after = lobster_interp::run(&lobster_ssa::destruct(&opted), func, args);
        match (before, after) {
            (Ok(a), Ok(b)) => assert_eq!((a.printed, a.returned), (b.printed, b.returned)),
            (Err(a), Err(b)) => assert_eq!((a.code, a.message), (b.code, b.message)),
            (a, b) => panic!("divergent outcomes: {a:?} vs {b:?}"),
        }
    }

    #[test]
    fn differential_all_examples_main() {
        for file in [
            "examples/hello.lobster",
            "examples/arithmetic.lobster",
            "examples/control_flow.lobster",
            "examples/structs_enums.lobster",
            "examples/strings.lobster",
            "examples/tomato_shapes.lobster",
        ] {
            let path = format!("../../{file}");
            let text = std::fs::read_to_string(&path).unwrap();
            run_both(&text, "main", Vec::new());
        }
    }

    #[test]
    fn differential_fib_and_loops() {
        run_both(
            "fn fib(n: u64) -> u64 {\n    if n < 2 {\n        return n;\n    }\n\n    fib(n - 1) + fib(n - 2)\n}\nfn main() {\n    println(fib(10));\n}\n",
            "main",
            Vec::new(),
        );
        run_both(
            "fn sum(n: u32) -> u32 {\n    let mut t = 0u32;\n    let mut i = 0u32;\n    loop {\n        if i >= n {\n            break;\n        }\n        t += i;\n        i += 1;\n    }\n    t\n}\nfn main() {\n    println(sum(100u32));\n}\n",
            "main",
            Vec::new(),
        );
    }

    #[test]
    fn differential_traps_preserved() {
        // Unused division by zero must still trap: trapping ops are never
        // removed, even when their value is unused.
        run_both(
            "fn main() {\n    let x = 1u32 / 0u32;\n    println(x);\n}\n",
            "main",
            Vec::new(),
        );
        // Unused shift/mask-heavy code keeps wrapping semantics.
        run_both(
            "fn f(a: u8) -> u8 {\n    let b = a + 1u8;\n    let c = b * 2u8;\n    c ^ 255u8\n}\nfn main() {\n    println(f(255u8));\n}\n",
            "main",
            Vec::new(),
        );
    }

    #[test]
    fn differential_match_and_casts() {
        run_both(
            "enum Dir { North, East, South, West, }\nfn turn(d: Dir) -> Dir {\n    match d {\n        North => East,\n        East => South,\n        South => West,\n        West => North,\n    }\n}\nfn is_east(d: Dir) -> bool {\n    match turn(d) {\n        East => true,\n        _ => false,\n    }\n}\nfn main() {\n    println(is_east(North));\n}\n",
            "main",
            Vec::new(),
        );
        run_both(
            "fn main() {\n    let x = 300u32;\n    let n = x as u8;\n    let f = n as f32;\n    println(n);\n    println(f);\n}\n",
            "main",
            Vec::new(),
        );
    }
}
