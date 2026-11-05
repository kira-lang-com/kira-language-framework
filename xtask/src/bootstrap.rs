//! Build KLF, bootstrap Kira, then install the KLF/Kira dev toolchains and
//! this checkout's `knvm` tools.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::paths::{framework, target_directory};
use crate::process::{run, run_with_env, step};
use crate::release;

const VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn command(arguments: &[String]) -> Result<(), String> {
    if !arguments.is_empty() {
        return Err("usage: cargo xtask bootstrap-kira".to_owned());
    }

    let workspace = framework();
    let checkout = languages_directory(&workspace)?.join("klf-kira");
    let kira_package = checkout.join("languages/kira");
    let knvm_package = checkout.join("tools/knvm");
    for package in [&kira_package, &knvm_package] {
        if !package.is_dir() {
            return Err(format!("missing bootstrap package {}", package.display()));
        }
    }

    let state_home = manager_home(&workspace)?;
    let output = target_directory(&workspace).join("bootstrap-kira");
    let feed_root = output.join("klf-feed");
    let feed_version = feed_root.join("dev").join(VERSION);
    let kira_bootstrap = output.join("kira-bootstrap");
    let manager_bootstrap = output.join(klf_target::executable("knvm-bootstrap"));

    step("Build KLF and make a local dev feed");
    release::command(&[
        feed_version.to_string_lossy().into_owned(),
        VERSION.to_owned(),
    ])?;

    let klf = feed_version
        .join("klf/bin")
        .join(klf_target::executable("klf"));
    require_file(&klf)?;

    step("Bootstrap the Kira driver and knvm");
    if kira_bootstrap.exists() && !kira_bootstrap.join("definition.lock").is_file() {
        std::fs::remove_dir_all(&kira_bootstrap).map_err(|error| {
            format!(
                "cannot clear incomplete bootstrap sysroot {}: {error}",
                kira_bootstrap.display()
            )
        })?;
    }
    let bootstrap_klf_home = feed_version.join("klf");
    let system_linker = OsString::from("1");
    let bootstrap_environment = [
        ("KLF_HOME", bootstrap_klf_home.as_os_str()),
        ("KLF_USE_SYSTEM_LINKER", system_linker.as_os_str()),
    ];
    run_with_env(
        &klf,
        [
            OsString::from("build"),
            kira_package.as_os_str().to_owned(),
            OsString::from("--out"),
            kira_bootstrap.as_os_str().to_owned(),
        ],
        &workspace,
        &bootstrap_environment,
    )?;

    let kira = kira_bootstrap
        .join("bin")
        .join(klf_target::executable("kira"));
    require_file(&kira)?;
    if manager_bootstrap.exists() {
        std::fs::remove_file(&manager_bootstrap).map_err(|error| {
            format!(
                "cannot replace bootstrap knvm {}: {error}",
                manager_bootstrap.display()
            )
        })?;
    }
    run(
        &kira,
        [
            OsString::from("build"),
            knvm_package.as_os_str().to_owned(),
            OsString::from("-o"),
            manager_bootstrap.as_os_str().to_owned(),
        ],
        &checkout,
    )?;
    require_file(&manager_bootstrap)?;

    let installed_klf = state_home.join("toolchains/klf/dev").join(VERSION);
    let environment = [
        ("PWD", checkout.as_os_str()),
        ("KLFD_HOME", state_home.as_os_str()),
        ("KNVM_KLF_RELEASE_DIR", feed_root.as_os_str()),
        ("KLF_HOME", installed_klf.as_os_str()),
        ("KLF_USE_SYSTEM_LINKER", system_linker.as_os_str()),
    ];

    step("Install KLF on the dev channel");
    if installed_klf.exists() {
        run_with_env(
            &manager_bootstrap,
            words(["klf", "uninstall", VERSION, "--channel", "dev"]),
            &checkout,
            &environment,
        )?;
    }
    run_with_env(
        &manager_bootstrap,
        words(["klf", "install", VERSION, "--channel", "dev"]),
        &checkout,
        &environment,
    )?;

    step("Build and install Kira on the dev channel");
    run_with_env(
        &manager_bootstrap,
        words(["kira", "binstall"]),
        &checkout,
        &environment,
    )?;

    step("Install knvm and compiler launchers from this checkout");
    run_with_env(
        &manager_bootstrap,
        words(["sinstall"]),
        &checkout,
        &environment,
    )?;

    println!("\nBootstrap installed KLF and Kira on `dev` and refreshed knvm.");
    println!("KLF dev: {}", installed_klf.display());
    println!(
        "Kira dev: {}",
        state_home.join("toolchains/kira/dev").display()
    );
    println!("knvm tools: {}", state_home.join("bin").display());
    Ok(())
}

fn languages_directory(workspace: &Path) -> Result<&Path, String> {
    workspace
        .parent()
        .ok_or_else(|| format!("{} has no parent languages directory", workspace.display()))
}

fn manager_home(workspace: &Path) -> Result<PathBuf, String> {
    if let Some(home) = std::env::var_os("KLFD_HOME").filter(|home| !home.is_empty()) {
        let path = PathBuf::from(home);
        return Ok(if path.is_absolute() {
            path
        } else {
            workspace.join(path)
        });
    }
    klf_target::home_dir()
        .map(|home| home.join(".klf"))
        .ok_or_else(|| "neither HOME nor USERPROFILE is set".to_owned())
}

fn require_file(path: &Path) -> Result<(), String> {
    if path.is_file() {
        Ok(())
    } else {
        Err(format!("expected build artifact at {}", path.display()))
    }
}

fn words<const N: usize>(words: [&str; N]) -> Vec<OsString> {
    words.into_iter().map(OsString::from).collect()
}
