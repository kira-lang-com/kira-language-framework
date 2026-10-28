use super::{ForeignClass, ForeignSignature};

/// Where one argument travels.
#[derive(Clone, Copy)]
pub(crate) enum Location {
    Integer(usize),
    Float(usize),
    Stack { offset: usize, size: usize },
}

/// Where each argument of a C signature travels under a convention.
#[derive(Clone)]
pub(crate) struct CallPlan {
    pub(crate) parameters: Vec<(ForeignClass, Location)>,
    pub(crate) result: Option<ForeignClass>,
    pub(super) integer_arity: Option<usize>,
    pub(super) f64_arity: Option<usize>,
    /// The bytes of stack the arguments take.
    pub(crate) stack_bytes: usize,
    pub(super) valid: bool,
}

/// How the host's C convention places arguments.
#[derive(Clone, Copy)]
pub(crate) enum Convention {
    /// System V and AAPCS64: integers and floats each fill their own
    /// registers in order, then the stack.
    Classed,
    /// Windows x64: each of the first four arguments takes the register of
    /// its position, the rest a stack word each.
    Positional,
}

/// The arguments Windows x64 passes in registers.
pub(crate) const POSITIONAL_REGISTERS: usize = 4;

impl Convention {
    pub(crate) const HOST: Self = if cfg!(all(target_arch = "x86_64", windows)) {
        Self::Positional
    } else {
        Self::Classed
    };
}

impl CallPlan {
    pub(crate) fn new(signature: &ForeignSignature) -> Self {
        Self::with(signature, Convention::HOST)
    }

    pub(crate) fn with(signature: &ForeignSignature, convention: Convention) -> Self {
        let placed = match convention {
            Convention::Classed => classed(signature),
            Convention::Positional => positional(signature),
        };
        Self::placed(signature, placed, true)
    }

    /// A call of a C function with a variadic tail, whose first `fixed`
    /// classes are its parameters (section 29.5). Apple's AAPCS64 passes
    /// each variadic argument on the stack in an 8-byte slot; Windows x64
    /// passes a variadic float among the first four arguments in the
    /// integer register of its position, where the callee's `va_arg` reads
    /// it; System V and Linux's AAPCS64 place the tail as any argument.
    /// On `x86_64` System V the caller must also set `al` to bound the vector
    /// registers the call uses. The trampoline does not set it, so every
    /// variadic plan there is invalid.
    pub(crate) fn variadic(signature: &ForeignSignature, fixed: usize) -> Self {
        Self::variadic_with(signature, fixed, Convention::HOST)
    }

    pub(crate) fn variadic_with(
        signature: &ForeignSignature,
        fixed: usize,
        convention: Convention,
    ) -> Self {
        let fixed = fixed.min(signature.parameters.len());
        let tail = &signature.parameters[fixed..];
        let float = |class: &ForeignClass| matches!(class, ForeignClass::F32 | ForeignClass::F64);
        let (placed, valid) = match convention {
            Convention::Positional => {
                let mut placed = positional(signature);
                for (position, (class, location)) in placed.iter_mut().enumerate().skip(fixed) {
                    if float(class) && position < POSITIONAL_REGISTERS {
                        *location = Location::Integer(position);
                    }
                }
                (placed, true)
            }
            Convention::Classed if cfg!(all(target_arch = "aarch64", target_vendor = "apple")) => {
                let mut placed = classed(&ForeignSignature {
                    parameters: signature.parameters[..fixed].to_vec(),
                    result: signature.result,
                });
                let mut stack = stack_end(&placed).next_multiple_of(8);
                for class in tail {
                    placed.push((
                        *class,
                        Location::Stack {
                            offset: stack,
                            size: 8,
                        },
                    ));
                    stack += 8;
                }
                (placed, true)
            }
            Convention::Classed => (classed(signature), !cfg!(target_arch = "x86_64")),
        };
        let mut plan = Self::placed(signature, placed, valid);
        // The fast paths call through a prototype of their own shape, which
        // places arguments as a call without a variadic tail does.
        plan.integer_arity = None;
        plan.f64_arity = None;
        plan
    }

    fn placed(
        signature: &ForeignSignature,
        placed: Vec<(ForeignClass, Location)>,
        valid: bool,
    ) -> Self {
        let floats = placed
            .iter()
            .filter(|(_, location)| matches!(location, Location::Float(_)))
            .count();
        let stack = stack_end(&placed);
        let integers = placed.len() - floats;
        let integer_arity = (floats == 0
            && stack == 0
            && !matches!(
                signature.result,
                Some(ForeignClass::F32 | ForeignClass::F64)
            ))
        .then_some(integers);
        let f64_arity = (signature.result == Some(ForeignClass::F64)
            && signature.parameters.len() <= 8
            && signature
                .parameters
                .iter()
                .all(|class| *class == ForeignClass::F64))
        .then_some(signature.parameters.len());
        Self {
            parameters: placed,
            result: signature.result,
            integer_arity,
            f64_arity,
            stack_bytes: stack,
            valid: valid && stack <= 256,
        }
    }

    /// Which of the first four arguments travel in floating-point registers
    /// under the positional convention, one bit per position.
    #[cfg(any(
        all(target_arch = "x86_64", windows),
        all(test, target_arch = "x86_64")
    ))]
    pub(crate) fn positional_shape(&self) -> usize {
        self.parameters
            .iter()
            .take(POSITIONAL_REGISTERS)
            .enumerate()
            .filter(|(_, (_, location))| matches!(location, Location::Float(_)))
            .fold(0, |shape, (position, _)| shape | 1 << position)
    }

    /// Whether the result travels in a floating-point register.
    pub(crate) fn float_result(&self) -> bool {
        matches!(self.result, Some(ForeignClass::F32 | ForeignClass::F64))
    }
}

/// The bytes of stack the placed arguments reach.
fn stack_end(placed: &[(ForeignClass, Location)]) -> usize {
    placed
        .iter()
        .map(|(_, location)| match location {
            Location::Stack { offset, size } => offset + size,
            Location::Integer(_) | Location::Float(_) => 0,
        })
        .max()
        .unwrap_or(0)
}

/// Windows x64 placement: the register of each of the first four positions,
/// integer or floating-point by the argument's class, then one stack word
/// per argument.
fn positional(signature: &ForeignSignature) -> Vec<(ForeignClass, Location)> {
    signature
        .parameters
        .iter()
        .enumerate()
        .map(|(position, class)| {
            let float = matches!(class, ForeignClass::F32 | ForeignClass::F64);
            let location = match position {
                position if position >= POSITIONAL_REGISTERS => Location::Stack {
                    offset: (position - POSITIONAL_REGISTERS) * 8,
                    size: 8,
                },
                position if float => Location::Float(position),
                position => Location::Integer(position),
            };
            (*class, location)
        })
        .collect()
}

/// System V and AAPCS64 placement: integers and floats each in their own
/// registers in order, then the stack, packed by size on Apple's AAPCS64.
fn classed(signature: &ForeignSignature) -> Vec<(ForeignClass, Location)> {
    let integer_registers = if cfg!(target_arch = "x86_64") { 6 } else { 8 };
    let (mut integers, mut floats, mut stack) = (0, 0, 0usize);
    signature
        .parameters
        .iter()
        .map(|class| {
            let float = matches!(class, ForeignClass::F32 | ForeignClass::F64);
            let location = if float && floats < 8 {
                let index = floats;
                floats += 1;
                Location::Float(index)
            } else if !float && integers < integer_registers {
                let index = integers;
                integers += 1;
                Location::Integer(index)
            } else {
                let size = if cfg!(all(target_arch = "aarch64", target_vendor = "apple")) {
                    match class {
                        ForeignClass::Signed(bits) | ForeignClass::Unsigned(bits) => {
                            usize::from((*bits).max(8).next_power_of_two() / 8).min(8)
                        }
                        ForeignClass::F32 => 4,
                        ForeignClass::F64 => 8,
                    }
                } else {
                    8
                };
                stack = stack.next_multiple_of(size);
                let offset = stack;
                stack += size;
                Location::Stack { offset, size }
            };
            (*class, location)
        })
        .collect()
}
