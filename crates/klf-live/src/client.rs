//! The client half of section 17.5: compile an app, connect to a device, say
//! hello, and push the app's modules.
//!
//! The client compiles to KIR and the device lowers it, so what crosses the
//! network is the portable form rather than the device's bytecode. That is what
//! lets one build serve a device on another KLF release.

use crate::transport::Connection;
use crate::wire::{Frame, Hello, Push};
use std::net::TcpStream;

/// What one pushed module produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Pushed {
    /// The device installed it and replaced this many functions.
    Installed(u32),
    /// The device refused it, with the reason and its own words.
    Refused(String),
}

/// A client's handle on a device.
pub struct Client {
    connection: Connection<TcpStream>,
}

impl Client {
    /// Connects to the device at `address`.
    ///
    /// # Errors
    /// Returns a message when nothing answers there.
    pub fn connect(address: &str) -> Result<Self, String> {
        Ok(Self {
            connection: Connection::dial(address)?,
        })
    }

    /// Opens the exchange: the client says hello first, and the device answers
    /// with its own hello carrying the layout of every module it holds.
    ///
    /// # Errors
    /// Returns a message when the device cannot be reached, answers something
    /// other than a hello, or speaks a different protocol major version.
    pub fn handshake(&mut self, bundle: &str) -> Result<Hello, String> {
        let opening = Hello {
            version: crate::wire::VERSION,
            klf_version: env!("CARGO_PKG_VERSION").to_owned(),
            bundle: bundle.to_owned(),
            layouts: Vec::new(),
        };
        let answer = self
            .connection
            .exchange(&Frame::Hello(opening))
            .map_err(|error| format!("the device did not answer the hello: {error}"))?;
        match answer {
            Frame::Hello(hello) => {
                if hello.version >> 8 != crate::wire::VERSION >> 8 {
                    return Err(format!(
                        "the device speaks protocol {}.{}, this client speaks {}",
                        hello.version >> 8,
                        hello.version & 0xff,
                        crate::wire::VERSION
                    ));
                }
                Ok(hello)
            }
            other => Err(format!("expected a hello, got {other:?}")),
        }
    }

    /// Pushes one module's KIR and reports what the device did with it.
    ///
    /// # Errors
    /// Returns a message when the exchange fails. A refusal is a result, not
    /// an error: the device answered, and what it said is the answer.
    pub fn push(&mut self, module: &str, kir: &str) -> Result<Pushed, String> {
        let answer = self.connection.exchange(&Frame::Push(Push {
            module: module.to_owned(),
            kir: kir.to_owned(),
        }))?;
        Ok(match answer {
            Frame::Ack(ack) => Pushed::Installed(ack.replaced),
            Frame::Nack(nack) => Pushed::Refused(format!("{}: {}", nack.reason, nack.message)),
            other => return Err(format!("expected an ack or a nack, got {other:?}")),
        })
    }

    /// Pushes `module` to a device whose hello was `hello`.
    ///
    /// The hello says what the device holds. A module it holds with another
    /// layout cannot be hot swapped, and finding that out before the push is
    /// cheaper than after it, so that is a refusal without a push.
    ///
    /// # Errors
    /// Returns a message when the exchange fails.
    pub fn push_module(
        &mut self,
        hello: &Hello,
        module: &klf_kir::Module,
    ) -> Result<Pushed, String> {
        let name = module.name.as_str();
        let ours = crate::wire::layout_hash(module);
        match hello.layout_of(name) {
            Some(theirs) if theirs != ours => Ok(Pushed::Refused(format!(
                "the device holds `{name}` with a different layout; restart it to adopt this one"
            ))),
            _ => self.push(name, &module.to_string()),
        }
    }
}
