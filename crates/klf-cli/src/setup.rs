//! `klf setup [--llvm]`: the newest released `klf` and the RIDL LLVM it links
//! with, installed, or brought up to date where they already are.
//!
//! A release of `kira-lang-com/kira-language-framework` publishes
//! `klf-<version>-<host>.tar.gz`, a distribution `klf dist` laid out, and
//! `kira-lang-com/llvm-ridl` publishes `llvm-ridl-<version>-<host>`, each
//! beside a `.sha256`. They land at `~/.klf/toolchains/klf/release/<version>`,
//! selected by `current.toml`, and `~/.klf/llvm-ridl/<version>`. `--llvm`
//! installs the LLVM alone, for a workspace build that lays out its own
//! distribution.
//!
//! What is replaced may be running, and Windows refuses to delete a running
//! image or the directory holding it, so the old one is renamed out of the
//! way and cleared by a later run. Nothing here needs Rust: `curl` and `tar`
//! ship with every host KLF runs on, Windows included.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const FRAMEWORK: &str = "kira-lang-com/kira-language-framework";
const LLVM: &str = "kira-lang-com/llvm-ridl";
const SET_ASIDE: &str = ".replaced-";

pub(crate) fn command(args: &[String]) -> ExitCode {
    let outcome = match args {
        [] => setup(),
        [flag] if flag == "--llvm" => llvm().map(|_| ()),
        _ => Err("usage: klf setup [--llvm]".to_owned()),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            klf_diag::err!("klf setup: {message}");
            ExitCode::FAILURE
        }
    }
}

fn setup() -> Result<(), String> {
    llvm()?;
    let home = klf_home()?;
    let release = fetch_text(&format!("https://api.github.com/repos/{FRAMEWORK}/releases/latest"))?;
    let release: serde_json::Value =
        serde_json::from_str(&release).map_err(|error| format!("the latest release does not read: {error}"))?;
    let tag = release["tag_name"].as_str().ok_or("the latest release has no tag")?;
    let version = tag.trim_start_matches('v');
    let channel = home.join("toolchains/klf/release");
    let installed = channel.join(version);
    let primary = installed.join("bin").join(klf_target::executable("klf"));
    if primary.exists() {
        klf_diag::out!("klf {version} is installed");
    } else {
        let asset = format!("klf-{version}-{}.tar.gz", klf_target::llvm_host_key());
        let url = format!("https://github.com/{FRAMEWORK}/releases/download/{tag}/{asset}");
        install(&url, &asset, &channel, &installed)?;
        klf_diag::out!("installed klf {version} at {}", installed.display());
    }
    let selection = format!(
        "channel = \"release\"\nversion = \"{version}\"\nprimary = \"{}\"\n",
        primary.display().to_string().replace('\\', "\\\\")
    );
    write(&home.join("toolchains/klf/current.toml"), &selection)?;
    on_path(&home, &primary)
}

/// The pinned RIDL LLVM, fetched unless it is installed.
fn llvm() -> Result<PathBuf, String> {
    let version = klf_target::LLVM_RIDL_VERSION;
    let root = klf_home()?.join("llvm-ridl").join(version);
    if root.join("bin").join(klf_target::executable("clang")).exists() {
        klf_diag::out!("llvm-ridl {version} is installed");
        return Ok(root);
    }
    let format = if cfg!(windows) { "zip" } else { "tar.xz" };
    let asset = format!("llvm-ridl-{version}-{}.{format}", klf_target::llvm_host_key());
    let url = format!("https://github.com/{LLVM}/releases/download/v{version}/{asset}");
    let parent = root.parent().ok_or("the LLVM root has no parent")?.to_path_buf();
    install(&url, &asset, &parent, &root)?;
    klf_diag::out!("installed llvm-ridl {version} at {}", root.display());
    Ok(root)
}

/// Downloads `url` and its `.sha256` into a staging directory under
/// `parent`, checks one against the other, unpacks it, and lands the tree at
/// `destination`.
fn install(url: &str, asset: &str, parent: &Path, destination: &Path) -> Result<(), String> {
    create(parent)?;
    sweep(parent);
    let staging = parent.join(format!("{SET_ASIDE}staging-{}", std::process::id()));
    create(&staging)?;
    let landed = unpack(url, asset, &staging).and_then(|tree| {
        set_aside(destination)?;
        std::fs::rename(&tree, destination).map_err(|error| format!("cannot land {}: {error}", destination.display()))
    });
    let _ = std::fs::remove_dir_all(&staging);
    sweep(parent);
    landed
}

fn unpack(url: &str, asset: &str, staging: &Path) -> Result<PathBuf, String> {
    let archive = staging.join(asset);
    tool("curl", &["--fail", "--location", "--silent", "--show-error", "--output", &text(&archive), url])?;
    let published = fetch_text(&format!("{url}.sha256"))?;
    let expected = published.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    let bytes = std::fs::read(&archive).map_err(|error| format!("cannot read {asset}: {error}"))?;
    let actual = klf_ridl::sha256::hex(&klf_ridl::sha256::sha256(&bytes));
    if actual != expected {
        return Err(format!("{asset} does not match its published checksum"));
    }
    let tree = staging.join("tree");
    create(&tree)?;
    tool("tar", &["-xf", &text(&archive), "-C", &text(&tree)])?;
    Ok(tree)
}

/// `~/.klf/bin/klf` becomes the installed `klf` when it is missing or is a
/// copy of one, and is left alone when it is something else that dispatches
/// to the selection.
fn on_path(home: &Path, primary: &Path) -> Result<(), String> {
    let bin = home.join("bin");
    create(&bin)?;
    sweep(&bin);
    let copy = bin.join(klf_target::executable("klf"));
    if !copy.exists() || copies_a_distribution(home, &copy) {
        set_aside(&copy)?;
        std::fs::copy(primary, &copy).map_err(|error| format!("cannot copy klf to {}: {error}", copy.display()))?;
    }
    sweep(&bin);
    klf_diag::out!("klf is {}; put {} on PATH", copy.display(), bin.display());
    Ok(())
}

/// Whether `file` is byte for byte the `klf` of an installed distribution.
fn copies_a_distribution(home: &Path, file: &Path) -> bool {
    let Ok(bytes) = std::fs::read(file) else { return false };
    let root = home.join("toolchains/klf");
    let binary = klf_target::executable("klf");
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .flat_map(|channel| std::fs::read_dir(channel.path()).into_iter().flatten().flatten())
        .map(|version| version.path().join("bin").join(&binary))
        .any(|installed| std::fs::read(installed).is_ok_and(|other| other == bytes))
}

fn klf_home() -> Result<PathBuf, String> {
    if let Some(home) = std::env::var_os("KLFD_HOME").filter(|home| !home.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    klf_target::home_dir()
        .map(|home| home.join(".klf"))
        .ok_or_else(|| "neither HOME nor USERPROFILE is set".to_owned())
}

fn fetch_text(url: &str) -> Result<String, String> {
    let output = Command::new("curl")
        .args(["--fail", "--location", "--silent", "--show-error", "--header", "User-Agent: klf", url])
        .output()
        .map_err(|error| format!("curl did not start: {error}"))?;
    if !output.status.success() {
        return Err(format!("{url}: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn tool(program: &str, arguments: &[&str]) -> Result<(), String> {
    let output = Command::new(program)
        .args(arguments)
        .output()
        .map_err(|error| format!("{program} did not start: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed: {}", String::from_utf8_lossy(&output.stderr).trim()))
    }
}

/// Moves `path` out of the way of its replacement, when it is there.
fn set_aside(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let aside = path.with_file_name(format!("{SET_ASIDE}{name}-{}", std::process::id()));
    std::fs::rename(path, &aside).map_err(|error| format!("cannot move {} aside: {error}", path.display()))
}

/// Deletes what earlier runs set aside, as far as the host allows: an image
/// still running stays until a later run.
fn sweep(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(SET_ASIDE) {
            let path = entry.path();
            let _ = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        }
    }
}

fn create(directory: &Path) -> Result<(), String> {
    std::fs::create_dir_all(directory).map_err(|error| format!("cannot create {}: {error}", directory.display()))
}

fn write(path: &Path, contents: &str) -> Result<(), String> {
    std::fs::write(path, contents).map_err(|error| format!("cannot write {}: {error}", path.display()))
}

fn text(path: &Path) -> String {
    path.display().to_string()
}
