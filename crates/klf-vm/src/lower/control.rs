//! Lowering of branches, switches and returns, including the block-argument
//! moves a jump has to perform before the jump itself.

use super::{LowerError, Lowerer};
use crate::bytecode::{Instr, Opcode};
use crate::program::{BranchCondition, Const};
use klf_core::Name;
use klf_kir::{BlockTarget, IntegerPredicate, Operation};

impl Lowerer<'_> {
    pub(super) fn lower_control(
        &mut self,
        operation: &Operation,
    ) -> Option<Result<(), LowerError>> {
        let result = match operation {
            Operation::SwitchInt(switch) => {
                self.lower_switch(&switch.value, &switch.cases, &switch.default)
            }
            Operation::SwitchEnum(switch) => {
                self.lower_enum_switch(&switch.value, &switch.cases, switch.default.as_ref())
            }
            Operation::Branch(target) => self.lower_branch(target),
            Operation::ConditionalBranch(branch) => {
                self.lower_cond_branch(&branch.condition, &branch.then_target, &branch.else_target)
            }
            Operation::Return(value) => self.lower_return(value.as_ref()),
            Operation::Fail(value) => {
                let register = match self.register(value) {
                    Ok(register) => register,
                    Err(error) => return Some(Err(error)),
                };
                self.emit(Instr::abc(Opcode::Fail, register, 0, 0));
                Ok(())
            }
            Operation::Unreachable => {
                self.emit(Instr::abc(Opcode::Unreach, 0, 0, 0));
                Ok(())
            }
            Operation::Trap(code) => self
                .pool(Const::String(code.clone()))
                .map(|index| self.emit(Instr::from_bx(Opcode::Trap, 0, index))),
            _ => return None,
        };
        Some(result)
    }

    fn lower_switch(
        &mut self,
        value: &Name,
        cases: &[(i128, BlockTarget)],
        default: &BlockTarget,
    ) -> Result<(), LowerError> {
        use crate::bytecode::Opcode;
        let selector = self.register(value)?;
        let width = self.width_of(value)?;
        for (key, target) in cases {
            let key_register = self.fresh()?;
            if let Ok(small) = i32::try_from(*key) {
                self.emit(Instr::from_bx(Opcode::LoadI, key_register, small));
            } else {
                let index = self.pool(Const::Int(*key))?;
                self.emit(Instr::from_bx(Opcode::LoadK, key_register, index));
            }
            let comparison = self.fresh()?;
            self.emit(
                Instr::abc(Opcode::Icmp, comparison, selector, key_register)
                    .with_flags(width.code()),
            );
            let site =
                self.add_branch_site(Some(BranchCondition::Register(comparison)), target, None)?;
            self.emit(Instr::from_bx(Opcode::BrIf, 0, site));
        }
        let site = self.add_branch_site(None, default, None)?;
        self.emit(Instr::from_bx(Opcode::Br, 0, site));
        Ok(())
    }

    fn lower_branch(&mut self, target: &BlockTarget) -> Result<(), LowerError> {
        let site = self.add_branch_site(None, target, None)?;
        self.emit(Instr::from_bx(Opcode::Br, 0, site));
        Ok(())
    }

    fn lower_cond_branch(
        &mut self,
        condition: &Name,
        then_target: &BlockTarget,
        else_target: &BlockTarget,
    ) -> Result<(), LowerError> {
        let cond = self.register(condition)?;
        let site = self.add_branch_site(
            Some(BranchCondition::Register(cond)),
            then_target,
            Some(else_target),
        )?;
        self.emit(Instr::from_bx(Opcode::BrIf, 0, site));
        Ok(())
    }

    pub(super) fn lower_compare_branch(
        &mut self,
        result: &Name,
        predicate: IntegerPredicate,
        lhs: &Name,
        rhs: &Name,
        then_target: &BlockTarget,
        else_target: &BlockTarget,
    ) -> Result<(), LowerError> {
        let condition = BranchCondition::IntegerCompare {
            result: self.register(result)?,
            lhs: self.register(lhs)?,
            rhs: self.register(rhs)?,
            width: self.width_of(lhs)?,
            predicate: super::integer_predicate_code(predicate),
        };
        let site = self.add_branch_site(Some(condition), then_target, Some(else_target))?;
        self.emit(Instr::from_bx(Opcode::BrCmp, 0, site));
        Ok(())
    }

    fn lower_return(&mut self, value: Option<&Name>) -> Result<(), LowerError> {
        if let Some(value) = value {
            let register = self.register(value)?;
            self.emit(Instr::abc(crate::bytecode::Opcode::RetV, register, 0, 0));
        } else {
            self.emit(Instr::abc(crate::bytecode::Opcode::Ret, 0, 0, 0));
        }
        Ok(())
    }
}
