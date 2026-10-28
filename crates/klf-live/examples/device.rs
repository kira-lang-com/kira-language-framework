//! A device on a socket, started with an app and answering pushes, so a
//! driver's `push` has something real to talk to. Bind it to a tailnet
//! address to push to it from another machine. Clients are served one after
//! another until the process is stopped.
//!
//! `cargo run -p klf-live --example device -- <host:port> <module.kir> [sysroot]`

use klf_live::wire::Frame;
use klf_live::{Bundle, Device, Engine, RunRequest, VmEngine};
use std::net::TcpListener;

fn main() {
    let mut arguments = std::env::args().skip(1);
    let address = arguments.next().unwrap_or_else(|| "127.0.0.1:0".to_owned());
    let path = arguments.next().expect("a module");
    let source = std::fs::read_to_string(&path).expect("reads");
    let module = klf_kir::parse_module(&source).expect("the module parses");

    let mut engine = VmEngine::new(std::env::temp_dir());
    if let Some(sysroot) = arguments.next() {
        engine = engine.with_sysroot(sysroot);
    }
    let bundle = Bundle {
        id: "dev.kira.live.example".to_owned(),
        debug: true,
    };
    let mut device = Device::new(engine, bundle);
    device.install(&module).expect("installs");
    let request = RunRequest::new();
    let started = device.engine_mut().run("main", &request).expect("runs");
    println!("main {started}");

    let listener = TcpListener::bind(&address).expect("binds");
    println!(
        "listening on {}",
        listener.local_addr().expect("has an address")
    );
    device.serve(
        &listener,
        env!("CARGO_PKG_VERSION"),
        &mut |device, answer| match answer {
            Frame::Ack(ack) => {
                println!("installed {} ({} functions)", ack.module, ack.replaced);
                match device.engine_mut().run("main", &request) {
                    Ok(outcome) => println!("main {outcome}"),
                    Err(error) => println!("main did not run: {error}"),
                }
            }
            Frame::Nack(nack) => println!("refused {}: {}", nack.module, nack.message),
            _ => {}
        },
        &mut |message| println!("client dropped: {message}"),
    );
}
