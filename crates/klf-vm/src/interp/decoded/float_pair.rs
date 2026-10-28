use super::{DecodedInstr, FloatMultiplyAdd};

pub(super) fn decode_float_pairs(code: &mut [DecodedInstr]) -> Vec<FloatMultiplyAdd> {
    let mut pairs = Vec::new();
    let mut pc = 0;
    while pc + 1 < code.len() {
        let DecodedInstr::FMul64 { a, b, c } = code[pc] else {
            pc += 1;
            continue;
        };
        let DecodedInstr::FAdd64 {
            a: add_a,
            b: add_b,
            c: add_c,
        } = code[pc + 1]
        else {
            pc += 1;
            continue;
        };
        if add_b != a && add_c != a {
            pc += 1;
            continue;
        }
        let Ok(index) = u32::try_from(pairs.len()) else {
            break;
        };
        pairs.push(FloatMultiplyAdd {
            multiply_a: a,
            multiply_b: b,
            multiply_c: c,
            add_a,
            add_b,
            add_c,
        });
        code[pc] = DecodedInstr::FloatMultiplyAdd(index);
        pc += 2;
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::super::super::{EvaluationBudget, Machine, Outcome};
    use super::super::decode_program;
    use super::*;
    use crate::program::{BcFunction, BranchSite, BranchTarget, Const, Program};
    use crate::{Capabilities, Instr, Opcode};

    #[test]
    fn float_pairs_keep_rounding_aliases_metering_and_branch_entry() {
        let x = 1.0 + 2.0f64.powi(-27);
        let y = 1.0 - 2.0f64.powi(-27);
        for (multiply_a, add_a, add_b, add_c) in [(3, 4, 3, 2), (0, 0, 0, 2), (2, 1, 2, 2)] {
            let program = Program {
                functions: vec![BcFunction {
                    name: "float_pair".into(),
                    register_count: 5,
                    param_count: 3,
                    returns_value: true,
                    code: vec![
                        Instr::abc(Opcode::FMul, multiply_a, 0, 1)
                            .with_flags(1)
                            .encode(),
                        Instr::abc(Opcode::FAdd, add_a, add_b, add_c)
                            .with_flags(1)
                            .encode(),
                        Instr::abc(Opcode::RetV, add_a, 0, 0).encode(),
                    ],
                    allocations: vec![],
                    call_sites: vec![],
                    branches: vec![],
                }],
                consts: vec![],
                globals: vec![],
                imports: vec![],
                initializers: vec![],
            };
            let mut registers = [x.to_bits(), y.to_bits(), (-1.0f64).to_bits(), 0, 0];
            let product = x * y;
            registers[multiply_a as usize] = product.to_bits();
            let expected = (f64::from_bits(registers[add_b as usize])
                + f64::from_bits(registers[add_c as usize]))
            .to_bits();
            let args = [x.to_bits(), y.to_bits(), (-1.0f64).to_bits()];
            assert!(matches!(
                decode_program(&program)[0].code[0],
                DecodedInstr::FloatMultiplyAdd(0)
            ));
            assert_eq!(
                Machine::new(&program).call(0, &args, &Capabilities::new()),
                Outcome::Return(Some(expected))
            );
            let mut budget = EvaluationBudget {
                steps_remaining: 3,
                memory_bytes: 4096,
            };
            let mut machine = Machine::for_evaluation(&program, &budget).unwrap();
            assert_eq!(
                machine.evaluate(0, &args, &mut budget),
                Outcome::Return(Some(expected))
            );
            assert_eq!(budget.steps_remaining, 0);
        }
        let program = Program {
            functions: vec![BcFunction {
                name: "enter_add".into(),
                register_count: 4,
                param_count: 2,
                returns_value: true,
                code: vec![
                    Instr::from_bx(Opcode::LoadK, 2, 0).encode(),
                    Instr::from_bx(Opcode::Br, 0, 0).encode(),
                    Instr::abc(Opcode::FMul, 2, 0, 1).with_flags(1).encode(),
                    Instr::abc(Opcode::FAdd, 3, 2, 0).with_flags(1).encode(),
                    Instr::abc(Opcode::RetV, 3, 0, 0).encode(),
                ],
                allocations: vec![],
                call_sites: vec![],
                branches: vec![BranchSite {
                    condition: None,
                    then_edge: BranchTarget {
                        target: 3,
                        moves: vec![],
                    },
                    else_edge: None,
                }],
            }],
            consts: vec![Const::Float(4.0f64.to_bits())],
            globals: vec![],
            imports: vec![],
            initializers: vec![],
        };
        assert!(matches!(
            decode_program(&program)[0].code[0],
            DecodedInstr::LoadConstant { .. }
        ));
        assert_eq!(
            Machine::new(&program).call(
                0,
                &[2.0f64.to_bits(), 3.0f64.to_bits()],
                &Capabilities::new()
            ),
            Outcome::Return(Some(6.0f64.to_bits()))
        );
    }
}
