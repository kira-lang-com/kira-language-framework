//! `cargo xtask release OUT VERSION`: this host's distribution, built with the
//! RIDL LLVM `klf setup --llvm` fetches and packed as
//! `OUT/klf-<version>-<host>.tar.gz` with its `.sha256`, beside the tree it
//! was packed from at `OUT/klf`.

use std::fmt::Write;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::paths::framework;
use crate::process::{run, step};

pub fn command(arguments: &[String]) -> Result<(), String> {
    let [out, version] = arguments else {
        return Err("usage: cargo xtask release OUT VERSION".to_owned());
    };
    std::fs::create_dir_all(out).map_err(|error| format!("cannot create {out}: {error}"))?;
    let out = std::fs::canonicalize(out).map_err(|error| error.to_string())?;
    step("klf");
    run("cargo", ["build", "--release", "--package", "klf-cli", "--bin", "klf"], &framework())?;
    let built = framework().join("target/release").join(klf_target::executable("klf"));
    run(&built, ["setup", "--llvm"], &framework())?;

    step("dist");
    let dist = out.join("klf");
    if dist.exists() {
        std::fs::remove_dir_all(&dist).map_err(|error| format!("cannot remove {}: {error}", dist.display()))?;
    }
    run(&built, ["dist".as_ref(), "--out".as_ref(), dist.as_os_str()], &framework())?;
    pack(&out, &format!("klf-{version}-{}.tar.gz", klf_target::llvm_host_key()), &dist)
}

/// `file` in `out`, of the tree whose members sit at its top level, with its
/// SHA-256 beside it as `<file>.sha256`.
pub fn pack(out: &Path, file: &str, tree: &Path) -> Result<(), String> {
    let archive = out.join(file);
    run(
        "tar",
        ["-czf".as_ref(), archive.as_os_str(), "-C".as_ref(), tree.as_os_str(), ".".as_ref()],
        out,
    )?;
    let bytes = std::fs::read(&archive).map_err(|error| format!("cannot read {file}: {error}"))?;
    let digest = sha256_hex(&bytes);
    std::fs::write(out.join(format!("{file}.sha256")), format!("{digest}  {file}\n"))
        .map_err(|error| format!("cannot write {file}.sha256: {error}"))?;
    println!("   packed {file} ({} MB)", bytes.len() / 1_000_000);
    Ok(())
}

/// The SHA-256 of `bytes`, as lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}
