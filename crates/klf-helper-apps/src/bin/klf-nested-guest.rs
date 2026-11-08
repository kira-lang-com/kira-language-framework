#![cfg(unix)]

use klf_helper_apps::attached_connection;
use klf_ridl_wire::{Reply, Value};
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let connection = Arc::new(attached_connection()?.ok_or("missing host connection")?);
    let host = Arc::clone(&connection);
    connection.serve_once(move |request| {
        assert_eq!(request.function, "Helper.compute");
        assert!(request.arguments.is_empty());
        host.call("Harmony.add", vec![Value::Uint(2), Value::Uint(40)])
            .unwrap_or_else(|error| Reply::Refused(error.to_string()))
    })?;
    connection.close()?;
    Ok(())
}
