//! Minimal installed plugin process exercising host-only RIDL imports.

use klf_helper_apps::attached_connection;
use klf_ridl_wire::{Reply, Value};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Installed plugins receive only the private runtime connection, not the
    // host application's environment or inherited process configuration.
    let inherited: Vec<_> = std::env::vars().map(|(name, _)| name).collect();
    if inherited
        .iter()
        .any(|name| name != "KLF_RIDL_CHANNEL_FD" && name != "KLF_RIDL_CONNECTION_ID")
    {
        return Err(format!("unexpected inherited environment: {inherited:?}").into());
    }
    let connection = attached_connection()?.ok_or("no host RIDL connection")?;
    let forbidden = connection.call("Harmony.deleteEverything", vec![])?;
    if !matches!(forbidden, Reply::Refused(_)) {
        return Err("an unexported host method was callable".into());
    }
    let result = connection.call("Harmony.add", vec![Value::Uint(2), Value::Uint(40)])?;
    if result != Reply::Returned(Value::Uint(42)) {
        return Err(format!("host RIDL call returned {result:?}").into());
    }
    connection.close()?;
    Ok(())
}
