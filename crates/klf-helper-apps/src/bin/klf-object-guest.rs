#![cfg(unix)]

use klf_helper_apps::attached_connection;
use klf_ridl_wire::Message;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = attached_connection()?.ok_or("missing host RIDL connection")?;
    connection.export_object(11)?;
    let event = connection.receive()?;
    if !matches!(event.message, Message::Released { object: 11, .. }) {
        return Err("the host did not release its RIDL proxy".into());
    }
    connection.close()?;
    Ok(())
}
