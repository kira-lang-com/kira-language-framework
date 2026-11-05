//! Section 11.6 pass 1: definite initialization for directly-addressed stack
//! slots. Slots that escape through aliases are deferred to the address
//! analysis; this pass must not guess their initialized state.

use tir_diag::{Area, Code, Diagnostic, Dialect};
use tir_format::{FunctionBody, TextInstruction};

const USE_BEFORE_INIT: Code = Code::new(Dialect::Tir, Area::Ownership, 14);
const DOUBLE_INIT: Code = Code::new(Dialect::Tir, Area::Ownership, 15);
const UNINIT: u8 = 1;
const INIT: u8 = 2;

enum Access {
    Allocate,
    Read,
    Take,
    Initialize,
    Assign,
    TrivialStore,
    Destroy,
    NoOp,
    Escaped,
}

/// Checks slots whose accesses use the allocation result directly. Remaining
/// aliases require the mandatory address/borrow analysis before T1 closes.
pub(crate) fn verify(body: &FunctionBody) -> Vec<Diagnostic> {
    let successors = crate::text_cfg::text_successors(body);
    let mut predecessors = vec![Vec::new(); body.blocks.len()];
    for (source, targets) in successors.iter().enumerate() {
        for &target in targets {
            predecessors[target].push(source);
        }
    }
    let slots = body
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter(|instruction| instruction.opcode == "stack.alloc")
        .flat_map(|instruction| &instruction.results)
        .collect::<Vec<_>>();
    let mut diagnostics = Vec::new();
    for slot in slots {
        let accesses = body
            .blocks
            .iter()
            .map(|block| {
                block
                    .instructions
                    .iter()
                    .map(|instruction| access(instruction, slot))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        if accesses
            .iter()
            .flatten()
            .any(|access| matches!(access, Access::Escaped))
        {
            continue;
        }
        let mut incoming = vec![0u8; body.blocks.len()];
        let mut outgoing = vec![0u8; body.blocks.len()];
        // Every independent CFG root starts with uninitialized stack storage.
        let roots = component_roots(&successors);
        let mut changed = true;
        while changed {
            changed = false;
            for block in 0..body.blocks.len() {
                let mut next = if roots[block] { UNINIT } else { 0 };
                for &predecessor in &predecessors[block] {
                    next |= outgoing[predecessor];
                }
                let mut state = next;
                for access in &accesses[block] {
                    state = transfer(state, access);
                }
                if next != incoming[block] || state != outgoing[block] {
                    incoming[block] = next;
                    outgoing[block] = state;
                    changed = true;
                }
            }
        }
        for (block, instructions) in body.blocks.iter().enumerate() {
            let mut state = incoming[block];
            for (instruction, access) in instructions.instructions.iter().zip(&accesses[block]) {
                match access {
                    Access::Read | Access::Take | Access::Destroy | Access::Assign
                        if state != INIT && state != 0 =>
                    {
                        diagnostics.push(Diagnostic::error(
                            USE_BEFORE_INIT,
                            format!("init.use-before-init: `%{slot}` is not initialized on every path"),
                        ).with_primary_span(instruction.span));
                    }
                    Access::Initialize if state & INIT != 0 => {
                        diagnostics.push(
                            Diagnostic::error(
                                DOUBLE_INIT,
                                format!("init.double-init: `%{slot}` may already be initialized"),
                            )
                            .with_primary_span(instruction.span),
                        );
                    }
                    _ => {}
                }
                state = transfer(state, access);
            }
        }
    }
    diagnostics
}

fn transfer(state: u8, access: &Access) -> u8 {
    if state == 0 {
        return if matches!(access, Access::Allocate) {
            UNINIT
        } else {
            state
        };
    }
    match access {
        Access::Allocate | Access::Take | Access::Destroy => UNINIT,
        Access::Initialize | Access::Assign | Access::TrivialStore => INIT,
        _ => state,
    }
}

fn access(instruction: &TextInstruction, slot: &str) -> Access {
    let uses = crate::ssa::text_value_uses(instruction)
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>();
    if instruction.opcode == "stack.alloc" && instruction.results.iter().any(|name| name == slot) {
        return Access::Allocate;
    }
    let selected = instruction.tokens.windows(3).find_map(|window| {
        match (&window[0].kind, &window[1].kind, &window[2].kind) {
            (
                tir_format::text::TokenKind::Punctuation('['),
                tir_format::text::TokenKind::Atom(value),
                tir_format::text::TokenKind::Punctuation(']'),
            ) if matches!(
                value.as_str(),
                "init" | "assign" | "trivial" | "take" | "copy"
            ) =>
            {
                Some(value.as_str())
            }
            _ => None,
        }
    });
    match instruction.opcode.as_str() {
        "load" | "load.borrow" if uses.as_slice() == [slot] => {
            if selected == Some("take") {
                Access::Take
            } else {
                Access::Read
            }
        }
        "store" if uses.len() == 2 && uses[1] == slot && uses[0] != slot => match selected {
            Some("init") => Access::Initialize,
            Some("assign") => Access::Assign,
            _ => Access::TrivialStore,
        },
        "copy.addr" | "move.addr" if uses.len() == 2 && uses[0] == slot && uses[1] != slot => {
            if instruction.opcode == "move.addr" {
                Access::Take
            } else {
                Access::Read
            }
        }
        "copy.addr" | "move.addr" if uses.len() == 2 && uses[1] == slot && uses[0] != slot => {
            if selected == Some("init") {
                Access::Initialize
            } else {
                Access::Assign
            }
        }
        "destroy.addr" if uses.as_slice() == [slot] => Access::Destroy,
        "stack.dealloc" if uses.as_slice() == [slot] => Access::NoOp,
        _ if uses.iter().any(|value| value == slot) => Access::Escaped,
        _ => Access::NoOp,
    }
}

fn component_roots(successors: &[Vec<usize>]) -> Vec<bool> {
    let mut reached = vec![false; successors.len()];
    let mut roots = vec![false; successors.len()];
    for start in 0..successors.len() {
        if reached[start] {
            continue;
        }
        roots[start] = true;
        reached[start] = true;
        let mut pending = vec![start];
        while let Some(block) = pending.pop() {
            for &target in &successors[block] {
                if !reached[target] {
                    reached[target] = true;
                    pending.push(target);
                }
            }
        }
    }
    roots
}

#[cfg(test)]
mod tests {
    use tir_format::parse_function_body;

    use super::*;

    fn errors(source: &str) -> Vec<Diagnostic> {
        verify(
            &parse_function_body(source)
                .expect("function parses")
                .expect("function body"),
        )
    }

    #[test]
    fn direct_slot_load_requires_definite_init() {
        let function = "func @test() -> unit {\nbb0:\n %slot = stack.alloc i32\n %value = load [trivial] %slot\n return\n}";
        assert!(errors(function).iter().any(|d| d.code == USE_BEFORE_INIT));
    }

    #[test]
    fn duplicate_initialization_is_rejected() {
        let function = "func @test(%v: i32) -> unit {\nbb0(%v: i32):\n %slot = stack.alloc i32\n store [init] %v to %slot\n store [init] %v to %slot\n return\n}";
        assert!(errors(function).iter().any(|d| d.code == DOUBLE_INIT));
    }

    #[test]
    fn all_paths_initialized_before_load() {
        let function = "func @test(%v: i32, %c: i1) -> unit {\nbb0(%v: i32, %c: i1):\n %slot = stack.alloc i32\n cond.br %c, bb1(), bb2()\nbb1:\n store [init] %v to %slot\n br bb2()\nbb2:\n %value = load [trivial] %slot\n return\n}";
        assert!(errors(function).iter().any(|d| d.code == USE_BEFORE_INIT));
    }

    #[test]
    fn initializing_all_branches_before_a_merge_is_accepted() {
        let function = "func @test(%v: i32, %c: i1) -> unit {\nbb0(%v: i32, %c: i1):\n %slot = stack.alloc i32\n cond.br %c, bb1(), bb2()\nbb1:\n store [init] %v to %slot\n br bb3()\nbb2:\n store [init] %v to %slot\n br bb3()\nbb3:\n %value = load [trivial] %slot\n return\n}";
        assert!(
            errors(function).is_empty(),
            "every incoming path is initialized"
        );
    }

    #[test]
    fn copy_and_move_address_source_must_be_initialized() {
        let function = "func @test() -> unit {\nbb0:\n %source = stack.alloc i32\n %destination = stack.alloc i32\n copy.addr %source to [init] %destination\n return\n}";
        assert!(errors(function).iter().any(|d| d.code == USE_BEFORE_INIT));
        let function = "func @test() -> unit {\nbb0:\n %source = stack.alloc i32\n %destination = stack.alloc i32\n move.addr %source to [init] %destination\n return\n}";
        assert!(errors(function).iter().any(|d| d.code == USE_BEFORE_INIT));
    }

    #[test]
    fn address_copy_init_then_move_consumes_source() {
        let function = "func @test(%v: i32) -> unit {\nbb0(%v: i32):\n %source = stack.alloc i32\n %destination = stack.alloc i32\n store [init] %v to %source\n copy.addr %source to [init] %destination\n move.addr %source to [assign] %destination\n %value = load [trivial] %source\n return\n}";
        let diagnostics = errors(function);
        assert_eq!(
            diagnostics
                .iter()
                .filter(|d| d.code == USE_BEFORE_INIT)
                .count(),
            1
        );
        assert!(
            diagnostics.iter().all(|d| d.code != DOUBLE_INIT),
            "expected valid copy init and move assign: {diagnostics:?}"
        );
    }

    #[test]
    fn initialized_slot_can_be_read_and_taken_once() {
        let function = "func @test(%v: i32) -> unit {\nbb0(%v: i32):\n %slot = stack.alloc i32\n store [init] %v to %slot\n %first = load [take] %slot\n %second = load [trivial] %slot\n return\n}";
        assert_eq!(
            errors(function)
                .iter()
                .filter(|d| d.code == USE_BEFORE_INIT)
                .count(),
            1
        );
    }
}
