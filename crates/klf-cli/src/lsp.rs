//! `klf lsp`: the language server for the kernel's own file types.

use klf_kernel_ide::{KernelAnalyzer, languages};
use klf_lsp::ServerConfig;
use std::process::ExitCode;
use std::sync::Arc;

pub fn command(args: &[String]) -> ExitCode {
    if let Some(argument) = args.first() {
        klf_diag::err!("klf lsp: unexpected argument `{argument}`");
        return ExitCode::FAILURE;
    }
    let config = ServerConfig {
        name: "klf".to_owned(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        languages: languages(),
        analyzer: Arc::new(KernelAnalyzer),
        formatter: None,
    };
    match klf_lsp::serve(config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            klf_diag::err!("klf lsp: {error}");
            ExitCode::FAILURE
        }
    }
}
