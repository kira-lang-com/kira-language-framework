/// Uniform-driver arguments returned by a Language compatibility hook.
///
/// The executable name is excluded. The generated frontend feeds these tokens
/// back through the uniform command-line parser.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DriverRequest {
    pub arguments: Vec<String>,
}

/// Selects the arguments the uniform driver parses after the compatibility hook.
#[must_use]
pub fn select_driver_arguments(original: &[String], request: Option<DriverRequest>) -> Vec<String> {
    request.map_or_else(|| original.to_vec(), |request| request.arguments)
}
