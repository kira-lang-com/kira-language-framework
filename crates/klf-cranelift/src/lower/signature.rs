//! The Cranelift signature of a KIR function.

use super::cl_type;
use crate::{EmitError, is_void};
use cranelift::codegen::ir::ArgumentExtension;
use cranelift::prelude::{AbiParam, Signature};
use cranelift_module::Module;
use cranelift_object::ObjectModule;
use klf_kir::{CType, Function};

/// The Cranelift signature for a KIR function.
pub(crate) fn signature(
    module: &mut ObjectModule,
    function: &Function,
) -> Result<Signature, EmitError> {
    let mut signature = module.make_signature();
    for parameter in &function.parameters {
        signature
            .params
            .push(AbiParam::new(cl_type(&parameter.ty)?));
    }
    if !is_void(&function.result) {
        signature
            .returns
            .push(AbiParam::new(cl_type(&function.result)?));
    }
    // D-118: a function that can `fail` takes the error out-pointer last.
    if function.failure.is_some() {
        let pointer = module.target_config().pointer_type();
        signature.params.push(AbiParam::new(pointer));
    }
    if let Some(prototype) = &function.c_prototype {
        let out = usize::from(matches!(prototype.result, CType::Value(_)));
        for (parameter, ty) in signature.params[out..]
            .iter_mut()
            .zip(&prototype.parameters)
        {
            parameter.extension = c_extension(ty);
        }
        if let Some(result) = signature.returns.first_mut() {
            result.extension = c_extension(&prototype.result);
        }
    }
    Ok(signature)
}

/// How C widens a narrow integer to a register: by its signedness, as every
/// supported ABI's callers or callees rely on.
fn c_extension(ty: &CType) -> ArgumentExtension {
    match ty {
        CType::Signed(bits) if *bits < 32 => ArgumentExtension::Sext,
        CType::Unsigned(bits) if *bits < 32 => ArgumentExtension::Uext,
        CType::Bool => ArgumentExtension::Uext,
        CType::Void
        | CType::Signed(_)
        | CType::Unsigned(_)
        | CType::Float(_)
        | CType::Pointer
        | CType::Value(_) => ArgumentExtension::None,
    }
}
