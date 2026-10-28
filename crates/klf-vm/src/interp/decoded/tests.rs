use super::super::{EvaluationBudget, Machine, Outcome};
use super::{DecodedInstr, decode_program};
use crate::bytecode::WidthCode;
use crate::program::{
    AllocShape, BcFunction, BranchCondition, BranchSite, BranchTarget, CallSite, Program,
};
use crate::{Capabilities, Instr, Opcode};
use klf_core::Name;

fn dense_switch_program() -> Program {
    let mut code = Vec::new();
    let mut branches = Vec::new();
    for case in 0..8u16 {
        let constant = 1 + case * 2;
        let result = constant + 1;
        code.push(Instr::from_bx(Opcode::LoadI, constant, i32::from(case)).encode());
        code.push(Instr::abc(Opcode::Icmp, result, 0, constant).encode());
        code.push(Instr::from_bx(Opcode::BrIf, 0, i32::from(case)).encode());
        branches.push(BranchSite {
            condition: Some(BranchCondition::Register(result)),
            then_edge: BranchTarget {
                target: 25 + usize::from(case),
                moves: Vec::new(),
            },
            else_edge: None,
        });
    }
    code.push(Instr::abc(Opcode::RetV, 15, 0, 0).encode());
    for case in 0..7u16 {
        code.push(Instr::abc(Opcode::RetV, 2 + case * 2, 0, 0).encode());
    }
    code.push(Instr::abc(Opcode::RetV, 13, 0, 0).encode());
    Program {
        functions: vec![BcFunction {
            name: Name::new("dense_switch"),
            register_count: 17,
            param_count: 1,
            returns_value: true,
            code,
            call_sites: Vec::new(),
            allocations: Vec::new(),
            branches,
        }],
        consts: Vec::new(),
        imports: Vec::new(),
        globals: Vec::new(),
        initializers: Vec::new(),
    }
}

#[test]
fn immediate_binary_keeps_metered_instruction_boundaries() {
    let program = Program {
        functions: vec![BcFunction {
            name: Name::new("immediate_add"),
            register_count: 3,
            param_count: 1,
            returns_value: true,
            code: vec![
                Instr::from_bx(Opcode::LoadI, 1, -7).encode(),
                Instr::abc(Opcode::Add, 2, 0, 1).with_flags(3).encode(),
                Instr::abc(Opcode::RetV, 2, 0, 0).encode(),
            ],
            call_sites: Vec::new(),
            allocations: Vec::new(),
            branches: Vec::new(),
        }],
        consts: Vec::new(),
        imports: Vec::new(),
        globals: Vec::new(),
        initializers: Vec::new(),
    };
    let decoded = decode_program(&program);
    assert!(matches!(decoded[0].code[0], DecodedInstr::LoadIBinary64(0)));
    assert!(matches!(decoded[0].code[1], DecodedInstr::Add64 { .. }));

    let mut machine = Machine::new(&program);
    assert_eq!(
        machine.call(0, &[19], &Capabilities::new()),
        Outcome::Return(Some(12))
    );

    let initial_budget = EvaluationBudget {
        steps_remaining: 3,
        memory_bytes: 4096,
    };
    let mut machine = Machine::for_evaluation(&program, &initial_budget).unwrap();
    let mut budget = EvaluationBudget {
        steps_remaining: 3,
        memory_bytes: 4096,
    };
    assert_eq!(
        machine.evaluate(0, &[19], &mut budget),
        Outcome::Return(Some(12))
    );
    assert_eq!(budget.steps_remaining, 0);
}

#[test]
fn floating_arithmetic_is_predecoded_and_preserves_metered_steps() {
    let program = Program {
        functions: vec![BcFunction {
            name: Name::new("float_add"),
            register_count: 3,
            param_count: 2,
            returns_value: true,
            code: vec![
                Instr::abc(Opcode::FAdd, 2, 0, 1).with_flags(1).encode(),
                Instr::abc(Opcode::RetV, 2, 0, 0).encode(),
            ],
            call_sites: Vec::new(),
            allocations: Vec::new(),
            branches: Vec::new(),
        }],
        consts: Vec::new(),
        imports: Vec::new(),
        globals: Vec::new(),
        initializers: Vec::new(),
    };
    let decoded = decode_program(&program);
    assert!(matches!(decoded[0].code[0], DecodedInstr::FAdd64 { .. }));

    let left = 3.25f64.to_bits();
    let right = 4.5f64.to_bits();
    let mut machine = Machine::new(&program);
    assert_eq!(
        machine.call(0, &[left, right], &Capabilities::new()),
        Outcome::Return(Some(7.75f64.to_bits()))
    );

    let initial_budget = EvaluationBudget {
        steps_remaining: 2,
        memory_bytes: 4096,
    };
    let mut machine = Machine::for_evaluation(&program, &initial_budget).unwrap();
    let mut budget = EvaluationBudget {
        steps_remaining: 2,
        memory_bytes: 4096,
    };
    assert_eq!(
        machine.evaluate(0, &[left, right], &mut budget),
        Outcome::Return(Some(7.75f64.to_bits()))
    );
    assert_eq!(budget.steps_remaining, 0);
}

#[test]
fn move_pair_preserves_order_and_metered_instruction_boundaries() {
    let program = Program {
        functions: vec![BcFunction {
            name: Name::new("move_pair"),
            register_count: 3,
            param_count: 0,
            returns_value: true,
            code: vec![
                Instr::from_bx(Opcode::LoadI, 0, 42).encode(),
                Instr::abc(Opcode::Mov, 1, 0, 0).encode(),
                Instr::abc(Opcode::Mov, 2, 1, 0).encode(),
                Instr::abc(Opcode::RetV, 2, 0, 0).encode(),
            ],
            call_sites: Vec::new(),
            allocations: Vec::new(),
            branches: Vec::new(),
        }],
        consts: Vec::new(),
        imports: Vec::new(),
        globals: Vec::new(),
        initializers: Vec::new(),
    };
    let decoded = decode_program(&program);
    assert!(matches!(decoded[0].code[1], DecodedInstr::MovePair(0)));
    assert!(matches!(decoded[0].code[2], DecodedInstr::Mov { .. }));

    let mut machine = Machine::new(&program);
    assert_eq!(
        machine.call(0, &[], &Capabilities::new()),
        Outcome::Return(Some(42))
    );

    let initial_budget = EvaluationBudget {
        steps_remaining: 4,
        memory_bytes: 4096,
    };
    let mut machine = Machine::for_evaluation(&program, &initial_budget).unwrap();
    let mut budget = initial_budget;
    assert_eq!(
        machine.evaluate(0, &[], &mut budget),
        Outcome::Return(Some(42))
    );
    assert_eq!(budget.steps_remaining, 0);
}

#[test]
fn alloca_pair_fuses_allocations_and_preserves_metered_steps() {
    let program = Program {
        functions: vec![BcFunction {
            name: Name::new("alloca_pair"),
            register_count: 4,
            param_count: 0,
            returns_value: true,
            code: vec![
                Instr::from_bx(Opcode::Alloca, 0, 0).encode(),
                Instr::from_bx(Opcode::Alloca, 1, 1).encode(),
                Instr::from_bx(Opcode::LoadI, 2, 73).encode(),
                Instr::abc(Opcode::Store, 0, 2, 0)
                    .with_flags(WidthCode::W64.code())
                    .encode(),
                Instr::abc(Opcode::Store, 1, 2, 0)
                    .with_flags(WidthCode::W64.code())
                    .encode(),
                Instr::abc(Opcode::Load, 3, 1, 0)
                    .with_flags(WidthCode::W64.code())
                    .encode(),
                Instr::abc(Opcode::RetV, 3, 0, 0).encode(),
            ],
            call_sites: Vec::new(),
            allocations: vec![
                AllocShape { size: 8, align: 8 },
                AllocShape { size: 16, align: 8 },
            ],
            branches: Vec::new(),
        }],
        consts: Vec::new(),
        imports: Vec::new(),
        globals: Vec::new(),
        initializers: Vec::new(),
    };
    let decoded = decode_program(&program);
    assert!(matches!(decoded[0].code[0], DecodedInstr::AllocaPair(0)));
    assert!(matches!(decoded[0].code[1], DecodedInstr::Alloca { .. }));

    let mut machine = Machine::new(&program);
    assert_eq!(
        machine.call(0, &[], &Capabilities::new()),
        Outcome::Return(Some(73))
    );

    let initial_budget = EvaluationBudget {
        steps_remaining: 7,
        memory_bytes: 1 << 20,
    };
    let mut machine = Machine::for_evaluation(&program, &initial_budget).unwrap();
    let mut budget = initial_budget;
    assert_eq!(
        machine.evaluate(0, &[], &mut budget),
        Outcome::Return(Some(73))
    );
    assert_eq!(budget.steps_remaining, 0);
}

#[test]
fn nested_frame_arena_preserves_caller_allocations() {
    let program = Program {
        functions: vec![
            BcFunction {
                name: Name::new("caller"),
                register_count: 3,
                param_count: 0,
                returns_value: true,
                code: vec![
                    Instr::from_bx(Opcode::Alloca, 0, 0).encode(),
                    Instr::from_bx(Opcode::LoadI, 1, 73).encode(),
                    Instr::abc(Opcode::Store, 0, 1, 0)
                        .with_flags(WidthCode::W64.code())
                        .encode(),
                    Instr::from_bx(Opcode::Call, 0, 0).encode(),
                    Instr::abc(Opcode::RetV, 2, 0, 0).encode(),
                ],
                call_sites: vec![CallSite {
                    callee: 1,
                    first_arg: 0,
                    arg_count: 1,
                    result: 2,
                    has_result: true,
                    foreign: None,
                    target: None,
                    fallible: None,
                }],
                allocations: vec![AllocShape { size: 8, align: 8 }],
                branches: Vec::new(),
            },
            BcFunction {
                name: Name::new("callee"),
                register_count: 4,
                param_count: 1,
                returns_value: true,
                code: vec![
                    Instr::from_bx(Opcode::Alloca, 1, 0).encode(),
                    Instr::from_bx(Opcode::LoadI, 2, 19).encode(),
                    Instr::abc(Opcode::Store, 1, 2, 0)
                        .with_flags(WidthCode::W64.code())
                        .encode(),
                    Instr::abc(Opcode::Load, 3, 0, 0)
                        .with_flags(WidthCode::W64.code())
                        .encode(),
                    Instr::abc(Opcode::RetV, 3, 0, 0).encode(),
                ],
                call_sites: Vec::new(),
                allocations: vec![AllocShape { size: 8, align: 8 }],
                branches: Vec::new(),
            },
        ],
        consts: Vec::new(),
        imports: Vec::new(),
        globals: Vec::new(),
        initializers: Vec::new(),
    };

    let mut machine = Machine::new(&program);
    assert_eq!(
        machine.call(0, &[], &Capabilities::new()),
        Outcome::Return(Some(73))
    );
    assert_eq!(
        machine.call(0, &[], &Capabilities::new()),
        Outcome::Return(Some(73))
    );
}

#[test]
fn add_load_fuses_address_calculation_and_keeps_metered_load() {
    let program = Program {
        functions: vec![BcFunction {
            name: Name::new("add_load"),
            register_count: 5,
            param_count: 1,
            returns_value: true,
            code: vec![
                Instr::from_bx(Opcode::Alloca, 1, 0).encode(),
                Instr::abc(Opcode::Store, 1, 0, 0)
                    .with_flags(WidthCode::W64.code())
                    .encode(),
                Instr::from_bx(Opcode::LoadI, 2, 0).encode(),
                Instr::abc(Opcode::Add, 3, 1, 2).with_flags(3).encode(),
                Instr::abc(Opcode::Load, 4, 3, 0)
                    .with_flags(WidthCode::W64.code())
                    .encode(),
                Instr::abc(Opcode::RetV, 4, 0, 0).encode(),
            ],
            call_sites: Vec::new(),
            allocations: vec![AllocShape { size: 8, align: 8 }],
            branches: Vec::new(),
        }],
        consts: Vec::new(),
        imports: Vec::new(),
        globals: Vec::new(),
        initializers: Vec::new(),
    };
    let decoded = decode_program(&program);
    assert!(matches!(decoded[0].code[3], DecodedInstr::AddLoad(0)));
    assert!(matches!(decoded[0].code[4], DecodedInstr::Load64 { .. }));

    let mut machine = Machine::new(&program);
    assert_eq!(
        machine.call(0, &[0x1234], &Capabilities::new()),
        Outcome::Return(Some(0x1234))
    );

    let initial_budget = EvaluationBudget {
        steps_remaining: 6,
        memory_bytes: 1 << 20,
    };
    let mut machine = Machine::for_evaluation(&program, &initial_budget).unwrap();
    let mut budget = initial_budget;
    assert_eq!(
        machine.evaluate(0, &[0x5678], &mut budget),
        Outcome::Return(Some(0x5678))
    );
    assert_eq!(budget.steps_remaining, 0);
}

#[test]
fn load_move_preserves_aliasing_and_metered_instruction_boundaries() {
    let program = Program {
        functions: vec![BcFunction {
            name: Name::new("load_move"),
            register_count: 4,
            param_count: 1,
            returns_value: true,
            code: vec![
                Instr::from_bx(Opcode::Alloca, 1, 0).encode(),
                Instr::abc(Opcode::Store, 1, 0, 0)
                    .with_flags(WidthCode::W64.code())
                    .encode(),
                Instr::abc(Opcode::Load, 2, 1, 0)
                    .with_flags(WidthCode::W64.code())
                    .encode(),
                Instr::abc(Opcode::Mov, 1, 2, 0).encode(),
                Instr::abc(Opcode::RetV, 1, 0, 0).encode(),
            ],
            call_sites: Vec::new(),
            allocations: vec![AllocShape { size: 8, align: 8 }],
            branches: Vec::new(),
        }],
        consts: Vec::new(),
        imports: Vec::new(),
        globals: Vec::new(),
        initializers: Vec::new(),
    };
    let decoded = decode_program(&program);
    assert!(matches!(
        decoded[0].code[2],
        DecodedInstr::Load64Move { .. }
    ));
    assert!(matches!(decoded[0].code[3], DecodedInstr::Mov { .. }));

    let mut machine = Machine::new(&program);
    assert_eq!(
        machine.call(0, &[0x1234], &Capabilities::new()),
        Outcome::Return(Some(0x1234))
    );

    let initial_budget = EvaluationBudget {
        steps_remaining: 5,
        memory_bytes: 1 << 20,
    };
    let mut machine = Machine::for_evaluation(&program, &initial_budget).unwrap();
    let mut budget = initial_budget;
    assert_eq!(
        machine.evaluate(0, &[0x5678], &mut budget),
        Outcome::Return(Some(0x5678))
    );
    assert_eq!(budget.steps_remaining, 0);
}

#[test]
fn tail_call_reuses_the_frame_and_preserves_metered_depth() {
    let program = Program {
        functions: vec![
            BcFunction {
                name: Name::new("forward"),
                register_count: 2,
                param_count: 0,
                returns_value: true,
                code: vec![
                    Instr::from_bx(Opcode::LoadI, 0, 99).encode(),
                    Instr::from_bx(Opcode::Call, 0, 0).encode(),
                    Instr::abc(Opcode::RetV, 1, 0, 0).encode(),
                ],
                call_sites: vec![CallSite {
                    callee: 1,
                    first_arg: 0,
                    arg_count: 1,
                    result: 1,
                    has_result: true,
                    foreign: None,
                    target: None,
                    fallible: None,
                }],
                allocations: Vec::new(),
                branches: Vec::new(),
            },
            BcFunction {
                name: Name::new("identity"),
                register_count: 1,
                param_count: 1,
                returns_value: true,
                code: vec![Instr::abc(Opcode::RetV, 0, 0, 0).encode()],
                call_sites: Vec::new(),
                allocations: Vec::new(),
                branches: Vec::new(),
            },
        ],
        consts: Vec::new(),
        imports: Vec::new(),
        globals: Vec::new(),
        initializers: Vec::new(),
    };
    let decoded = decode_program(&program);
    assert!(matches!(decoded[0].code[1], DecodedInstr::TailCall { .. }));
    assert!(matches!(decoded[0].code[2], DecodedInstr::ReturnValue(1)));

    let mut machine = Machine::new(&program);
    assert_eq!(
        machine.call(0, &[], &Capabilities::new()),
        Outcome::Return(Some(99))
    );

    let initial_budget = EvaluationBudget {
        steps_remaining: 4,
        memory_bytes: 4096,
    };
    let mut machine = Machine::for_evaluation(&program, &initial_budget).unwrap();
    let mut budget = initial_budget;
    assert_eq!(
        machine.evaluate(0, &[], &mut budget),
        Outcome::Return(Some(99))
    );
    assert_eq!(budget.steps_remaining, 0);
}

#[test]
fn decoded_instruction_is_one_word() {
    assert_eq!(std::mem::size_of::<DecodedInstr>(), 8);
}

#[test]
fn out_of_range_jump_lands_on_the_end_sentinel() {
    let program = Program {
        functions: vec![BcFunction {
            name: Name::new("out_of_range_jump"),
            register_count: 0,
            param_count: 0,
            returns_value: false,
            code: vec![Instr::from_bx(Opcode::Jmp, 0, 99).encode()],
            call_sites: Vec::new(),
            allocations: Vec::new(),
            branches: Vec::new(),
        }],
        consts: Vec::new(),
        imports: Vec::new(),
        globals: Vec::new(),
        initializers: Vec::new(),
    };
    let decoded = decode_program(&program);
    assert!(matches!(decoded[0].code[1], DecodedInstr::End));

    let mut machine = Machine::new(&program);
    assert_eq!(
        machine.call(0, &[], &Capabilities::new()),
        Outcome::Trap("KLF-VM-0002".to_owned())
    );
}

#[test]
fn dense_equality_switch_dispatches_directly_and_preserves_metered_code() {
    let program = dense_switch_program();
    let decoded = decode_program(&program);
    assert!(matches!(decoded[0].code[0], DecodedInstr::EnumSwitch(0)));
    assert_eq!(decoded[0].enum_switches[0].cases.len(), 8);

    let mut machine = Machine::new(&program);
    for (tag, expected) in [(0, 1), (3, 1), (7, 6), (8, 7), (256, 1)] {
        assert_eq!(
            machine.call(0, &[tag], &Capabilities::new()),
            Outcome::Return(Some(expected))
        );
    }

    let initial_budget = EvaluationBudget {
        steps_remaining: 128,
        memory_bytes: 4096,
    };
    let mut machine = Machine::for_evaluation(&program, &initial_budget).unwrap();
    let mut budget = EvaluationBudget {
        steps_remaining: 128,
        memory_bytes: 4096,
    };
    assert_eq!(
        machine.evaluate(0, &[7], &mut budget),
        Outcome::Return(Some(6))
    );
}

#[test]
fn immediate_memory_fusion_preserves_widths_aliases_traps_and_metering() {
    for width in 0..4 {
        for offset in [0, 8, i32::MAX] {
            let program = Program {
                functions: vec![BcFunction {
                    name: "offset_memory".into(),
                    register_count: 5,
                    param_count: 1,
                    returns_value: true,
                    allocations: vec![AllocShape { size: 16, align: 8 }],
                    call_sites: vec![],
                    branches: vec![],
                    code: vec![
                        Instr::from_bx(Opcode::Alloca, 1, 0).encode(),
                        Instr::from_bx(Opcode::LoadI, 4, offset).encode(),
                        Instr::abc(Opcode::Add, 2, 1, 4).with_flags(3).encode(),
                        Instr::abc(Opcode::Store, 2, 0, 0)
                            .with_flags(width)
                            .encode(),
                        Instr::from_bx(Opcode::LoadI, 4, offset).encode(),
                        Instr::abc(Opcode::Add, 2, 1, 4).with_flags(3).encode(),
                        Instr::abc(Opcode::Load, 2, 2, 0).with_flags(width).encode(),
                        Instr::abc(Opcode::RetV, 2, 0, 0).encode(),
                    ],
                }],
                consts: vec![],
                imports: vec![],
                globals: vec![],
                initializers: vec![],
            };
            let decoded = decode_program(&program);
            assert!(matches!(
                decoded[0].code[1],
                DecodedInstr::LoadIBinaryMemory64(0)
            ));
            assert!(matches!(
                decoded[0].code[4],
                DecodedInstr::LoadIBinaryMemory64(1)
            ));
            let expected = if offset == i32::MAX {
                Outcome::Trap("KLF-RT-0005".into())
            } else {
                Outcome::Return(Some(
                    0xfedc_ba98_7654_3210 & (u64::MAX >> (64 - (8 << width))),
                ))
            };
            let mut machine = Machine::new(&program);
            assert_eq!(
                machine.call(0, &[0xfedc_ba98_7654_3210], &Capabilities::new()),
                expected
            );
            let mut budget = EvaluationBudget {
                steps_remaining: 8,
                memory_bytes: 1024 * 1024,
            };
            let mut machine = Machine::for_evaluation(&program, &budget).unwrap();
            assert_eq!(
                machine.evaluate(0, &[0xfedc_ba98_7654_3210], &mut budget),
                expected
            );
            assert_eq!(
                budget.steps_remaining,
                if offset == i32::MAX { 4 } else { 0 }
            );
        }
    }
}

#[test]
fn direct_float64_arithmetic_preserves_aliases_and_special_values() {
    for op in [Opcode::FAdd, Opcode::FSub, Opcode::FMul, Opcode::FDiv] {
        let program = Program {
            functions: vec![BcFunction {
                name: "float64".into(),
                register_count: 2,
                param_count: 2,
                returns_value: true,
                code: vec![
                    Instr::abc(op, 0, 0, 1).with_flags(1).encode(),
                    Instr::abc(Opcode::RetV, 0, 0, 0).encode(),
                ],
                call_sites: vec![],
                allocations: vec![],
                branches: vec![],
            }],
            consts: vec![],
            imports: vec![],
            globals: vec![],
            initializers: vec![],
        };
        for lhs in [
            0.0f64,
            -0.0,
            1.0,
            -13.5,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
        ] {
            for rhs in [0.0f64, -0.0, 2.5, f64::INFINITY, f64::NAN] {
                let args = [lhs.to_bits(), rhs.to_bits()];
                let expected = crate::numeric::float_binary(op, args[0], args[1], true);
                let mut machine = Machine::new(&program);
                assert_eq!(
                    machine.call(0, &args, &Capabilities::new()),
                    Outcome::Return(Some(expected))
                );
            }
        }
    }
}
