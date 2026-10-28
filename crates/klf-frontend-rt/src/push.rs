//! `<driver> push <host:port>`: the hot module push client of section 5.5.
//!
//! The driver has already compiled the package to KIR, the portable form a
//! device lowers itself (section 17.5), so what is left is the exchange: say
//! hello, push the module, and report what the device said.

use klf_defsvc::FrontendMetadata;
use klf_kir::Module;
use klf_live::{Client, Pushed};

/// Pushes `module` to the device at `device`. Returns the exit status: zero
/// when the device installed it, one when it refused.
///
/// # Errors
/// Returns a message when the device cannot be reached or the exchange fails.
pub(crate) fn push(
    metadata: &FrontendMetadata,
    module: &Module,
    device: &str,
) -> Result<i32, String> {
    let name = module.name.as_str();
    let mut client = Client::connect(device)?;
    let hello = client.handshake(&metadata.driver)?;
    klf_diag::out!("Hello: Ok {} {}", hello.bundle, hello.klf_version);
    match client.push_module(&hello, module)? {
        Pushed::Installed(replaced) => {
            klf_diag::out!("Pushed: Ok {name} ({replaced} functions)");
            Ok(0)
        }
        Pushed::Refused(message) => {
            klf_diag::err!("Pushed: Failed {name}: {message}");
            Ok(1)
        }
    }
}
