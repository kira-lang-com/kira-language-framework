use super::Lowerer;
use crate::ty::FeType;
use klf_core::Name;
use klf_kir::{ConvertKind, Type};
use klf_services::ServiceType;
use std::cmp::Ordering;

impl Lowerer<'_> {
    pub(crate) fn lower_service_call(
        &mut self,
        service: &klf_services::KernelService,
        arguments: Vec<Name>,
        result: FeType,
    ) -> (Name, FeType) {
        let narrowed: Vec<Name> = arguments
            .into_iter()
            .zip(&service.signature.parameters)
            .map(|(value, parameter)| match parameter {
                ServiceType::Integer { bits, .. } if *bits < 64 => {
                    self.fb
                        .convert(ConvertKind::Truncate, value, Type::Integer(*bits))
                }
                _ => value,
            })
            .collect();
        if result == FeType::Void {
            self.fb
                .call_foreign_void(Name::new(&service.symbol), narrowed);
            return (self.fb.const_int(Type::Integer(64), 0), result);
        }
        let returned = self.fb.call_foreign(Name::new(&service.symbol), narrowed);
        if result == FeType::Bool {
            let adapted = self
                .fb
                .convert(ConvertKind::Truncate, returned, Type::Integer(1));
            return (adapted, result);
        }
        let ServiceType::Integer {
            bits: abi_bits,
            signed: abi_signed,
        } = service.signature.result
        else {
            return (returned, result);
        };
        let FeType::Int {
            bits: result_bits, ..
        } = result
        else {
            return (returned, result);
        };
        let adapted = match result_bits.cmp(&abi_bits) {
            Ordering::Equal => returned,
            Ordering::Less => self
                .fb
                .convert(ConvertKind::Truncate, returned, result.to_kir()),
            Ordering::Greater => self.fb.convert(
                if abi_signed {
                    ConvertKind::SignExtend
                } else {
                    ConvertKind::ZeroExtend
                },
                returned,
                result.to_kir(),
            ),
        };
        (adapted, result)
    }
}
