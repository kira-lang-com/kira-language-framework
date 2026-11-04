use crate::{ClosureType, FunctionType, ParameterType, Type};

pub(crate) fn contains_generic(ty: &Type) -> bool {
    match ty {
        Type::Generic(_) => true,
        Type::FixedVector { element, .. }
        | Type::ScalableVector(element)
        | Type::Reference(element)
        | Type::Unique(element)
        | Type::Weak(element)
        | Type::Address {
            pointee: element, ..
        }
        | Type::Tensor { element, .. } => contains_generic(element),
        Type::Tuple(elements) => elements.iter().any(contains_generic),
        Type::Array { element, .. } => contains_generic(element),
        Type::Nominal { arguments, .. } => arguments.iter().any(contains_generic),
        Type::Function(signature) => signature_contains_generic(signature),
        Type::Closure(closure) => signature_contains_generic(&closure.signature),
        Type::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => {
            contains_generic(yield_type)
                || contains_generic(resume_type)
                || contains_generic(result_type)
        }
        Type::Texture { component, .. } => contains_generic(component),
        Type::VisibleFunctionTable(signature) => signature_contains_generic(signature),
        Type::CooperativeMatrix { element, .. } => contains_generic(element),
        Type::Entry {
            root, immediate, ..
        } => contains_generic(root) || contains_generic(immediate),
        Type::Integer(_)
        | Type::Float(_)
        | Type::Pointer(_)
        | Type::Code
        | Type::Unit
        | Type::Never
        | Type::Frame(_)
        | Type::Sampler
        | Type::AccelerationStructure
        | Type::RayQuery
        | Type::HitObject => false,
    }
}

pub(crate) fn paths_qualified(ty: &Type) -> bool {
    match ty {
        Type::FixedVector { element, .. }
        | Type::ScalableVector(element)
        | Type::Reference(element)
        | Type::Unique(element)
        | Type::Weak(element)
        | Type::Address {
            pointee: element, ..
        }
        | Type::Tensor { element, .. } => paths_qualified(element),
        Type::Tuple(elements) => elements.iter().all(paths_qualified),
        Type::Array { element, .. } => paths_qualified(element),
        Type::Nominal { path, arguments } => {
            path.contains('.') && arguments.iter().all(paths_qualified)
        }
        Type::Function(signature) => signature_paths_qualified(signature),
        Type::Closure(closure) => signature_paths_qualified(&closure.signature),
        Type::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => {
            paths_qualified(yield_type)
                && paths_qualified(resume_type)
                && paths_qualified(result_type)
        }
        Type::Frame(path) => path.contains('.'),
        Type::Texture { component, .. } => paths_qualified(component),
        Type::VisibleFunctionTable(signature) => signature_paths_qualified(signature),
        Type::CooperativeMatrix { element, .. } => paths_qualified(element),
        Type::Entry {
            root, immediate, ..
        } => paths_qualified(root) && paths_qualified(immediate),
        Type::Integer(_)
        | Type::Float(_)
        | Type::Pointer(_)
        | Type::Code
        | Type::Unit
        | Type::Never
        | Type::Generic(_)
        | Type::Sampler
        | Type::AccelerationStructure
        | Type::RayQuery
        | Type::HitObject => true,
    }
}

fn signature_paths_qualified(signature: &FunctionType) -> bool {
    signature
        .parameters
        .iter()
        .all(|parameter| paths_qualified(&parameter.ty))
        && paths_qualified(&signature.result)
        && signature
            .failure
            .as_ref()
            .is_none_or(|failure| paths_qualified(failure))
}

fn signature_contains_generic(signature: &FunctionType) -> bool {
    signature
        .parameters
        .iter()
        .any(|parameter| contains_generic(&parameter.ty))
        || contains_generic(&signature.result)
        || signature
            .failure
            .as_ref()
            .is_some_and(|failure| contains_generic(failure))
}

pub(crate) fn qualify(ty: &Type, module_path: &str) -> Type {
    match ty {
        Type::FixedVector { lanes, element } => Type::FixedVector {
            lanes: *lanes,
            element: Box::new(qualify(element, module_path)),
        },
        Type::ScalableVector(element) => {
            Type::ScalableVector(Box::new(qualify(element, module_path)))
        }
        Type::Tuple(elements) => Type::Tuple(
            elements
                .iter()
                .map(|element| qualify(element, module_path))
                .collect(),
        ),
        Type::Array { count, element } => Type::Array {
            count: *count,
            element: Box::new(qualify(element, module_path)),
        },
        Type::Nominal { path, arguments } => Type::Nominal {
            path: qualify_path(path, module_path),
            arguments: arguments
                .iter()
                .map(|argument| qualify(argument, module_path))
                .collect(),
        },
        Type::Reference(inner) => Type::Reference(Box::new(qualify(inner, module_path))),
        Type::Unique(inner) => Type::Unique(Box::new(qualify(inner, module_path))),
        Type::Weak(inner) => Type::Weak(Box::new(qualify(inner, module_path))),
        Type::Address { pointee, space } => Type::Address {
            pointee: Box::new(qualify(pointee, module_path)),
            space: *space,
        },
        Type::Function(signature) => Type::Function(qualify_signature(signature, module_path)),
        Type::Closure(closure) => Type::Closure(ClosureType {
            borrowed: closure.borrowed,
            signature: qualify_signature(&closure.signature, module_path),
        }),
        Type::Coroutine {
            yield_type,
            resume_type,
            result_type,
        } => Type::Coroutine {
            yield_type: Box::new(qualify(yield_type, module_path)),
            resume_type: Box::new(qualify(resume_type, module_path)),
            result_type: Box::new(qualify(result_type, module_path)),
        },
        Type::Frame(path) => Type::Frame(qualify_path(path, module_path)),
        Type::Texture {
            dimension,
            component,
            access,
            multisampled,
        } => Type::Texture {
            dimension: *dimension,
            component: Box::new(qualify(component, module_path)),
            access: *access,
            multisampled: *multisampled,
        },
        Type::VisibleFunctionTable(signature) => {
            Type::VisibleFunctionTable(Box::new(qualify_signature(signature, module_path)))
        }
        Type::Tensor { count, element } => Type::Tensor {
            count: *count,
            element: Box::new(qualify(element, module_path)),
        },
        Type::CooperativeMatrix {
            scope,
            rows,
            columns,
            element,
            use_kind,
        } => Type::CooperativeMatrix {
            scope: *scope,
            rows: *rows,
            columns: *columns,
            element: Box::new(qualify(element, module_path)),
            use_kind: *use_kind,
        },
        Type::Entry {
            kind,
            root,
            immediate,
        } => Type::Entry {
            kind: *kind,
            root: Box::new(qualify(root, module_path)),
            immediate: Box::new(qualify(immediate, module_path)),
        },
        Type::Integer(_)
        | Type::Float(_)
        | Type::Pointer(_)
        | Type::Code
        | Type::Unit
        | Type::Never
        | Type::Generic(_)
        | Type::Sampler
        | Type::AccelerationStructure
        | Type::RayQuery
        | Type::HitObject => ty.clone(),
    }
}

fn qualify_signature(signature: &FunctionType, module_path: &str) -> FunctionType {
    FunctionType {
        parameters: signature
            .parameters
            .iter()
            .map(|parameter| ParameterType {
                context: parameter.context,
                convention: parameter.convention,
                ty: qualify(&parameter.ty, module_path),
            })
            .collect(),
        result: Box::new(qualify(&signature.result, module_path)),
        failure: signature
            .failure
            .as_ref()
            .map(|failure| Box::new(qualify(failure, module_path))),
        asynchronous: signature.asynchronous,
        abi: signature.abi,
        foreign: signature.foreign,
    }
}

fn qualify_path(path: &str, module_path: &str) -> String {
    if path.contains('.') || module_path.is_empty() {
        path.to_owned()
    } else {
        format!("{module_path}.{path}")
    }
}
