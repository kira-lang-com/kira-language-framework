//! Lowering of `try.call` (sections 12.11 and 15.5).
//!
//! The call site names the register that says whether the callee failed and
//! the branch site holding the error and normal targets. The call writes the
//! result, or on failure the error, to one register; the branch then reads
//! the flag and hands that register to the target block as its leading
//! parameter.

use super::{BranchFixup, LowerError, Lowerer, err};
use crate::bytecode::{Instr, Opcode};
use crate::program::{BranchCondition, BranchSite, CallSite, FallibleSite};
use klf_kir::{FallibleCall, Type};

impl Lowerer<'_> {
    pub(super) fn lower_try_call(&mut self, call: &FallibleCall) -> Result<(), LowerError> {
        let index = *self
            .indices
            .get(call.callee.as_str())
            .ok_or_else(|| err(format!("`try.call` to unknown function `@{}`", call.callee)))?;
        let returns_value = self
            .functions
            .get(call.callee.as_str())
            .is_some_and(|function| !matches!(function.result, Type::Unit | Type::Never));
        let first_arg = self.stage_arguments(&call.arguments)?;
        let arg_count = u16::try_from(call.arguments.len())
            .map_err(|_| err("try.call has too many arguments"))?;
        let value = self.fresh()?;
        let failed = self.fresh()?;

        let error_edge = self.delivering_edge(&call.error, &[value])?;
        let normal_delivered: &[u16] = if returns_value { &[value] } else { &[] };
        let normal_edge = self.delivering_edge(&call.normal, normal_delivered)?;
        let branch = self.branches.len();
        let branch_index = i32::try_from(branch).map_err(|_| err("too many branch sites"))?;
        self.branches.push(BranchSite {
            condition: Some(BranchCondition::Register(failed)),
            then_edge: error_edge,
            else_edge: Some(normal_edge),
        });
        self.branch_fixups.push(BranchFixup {
            site: branch,
            then_block: call.error.block.clone(),
            else_block: Some(call.normal.block.clone()),
        });

        let site = u16::try_from(self.call_sites.len()).map_err(|_| err("too many call sites"))?;
        self.call_sites.push(CallSite {
            callee: index,
            first_arg,
            arg_count,
            result: value,
            has_result: true,
            foreign: None,
            target: None,
            fallible: Some(FallibleSite {
                failed,
                branch: branch
                    .try_into()
                    .map_err(|_| err("too many branch sites"))?,
            }),
        });
        self.emit(Instr::from_bx(Opcode::Call, value, i32::from(site)));
        self.emit(Instr::from_bx(Opcode::BrIf, 0, branch_index));
        Ok(())
    }
}
