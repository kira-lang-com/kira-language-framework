//! Resource-transfer fixture: a host grants one descriptor to this plugin.

#![cfg(unix)]

use klf_helper_apps::attached_connection;
use klf_ridl_wire::{Message, Reply, Value};
use std::fs::File;
use std::io::Read;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = attached_connection()?.ok_or("missing inherited host connection")?;
    let incoming = connection.receive()?;
    let Message::Exported { object: 7, .. } = incoming.message else {
        return Err("the plugin received an unexpected resource".into());
    };
    let mut file: File = incoming
        .resource
        .ok_or("no OS resource was transferred")?
        .into();
    let mut contents = Vec::new();
    file.read_to_end(&mut contents)?;
    let answer = connection.call("Harmony.resourceBytes", vec![Value::Bytes(contents)])?;
    if answer != Reply::Returned(Value::Void) {
        return Err("host did not acknowledge the shared resource".into());
    }
    connection.release(7)?;
    connection.close()?;
    Ok(())
}
