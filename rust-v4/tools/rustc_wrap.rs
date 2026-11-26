//! Lightweight pure-Rust `rustc-wrapper` for `rust-v4`:
//! 1. Injects `-C dlltool=<abs>/coff-dlltool.exe` for `raw-dylib` imports on `x86_64-pc-windows-gnu`
//!    without requiring external MSYS2/MinGW `as.exe` or MSVC Build Tools.
//! 2. Dynamically injects `--remap-path-prefix` for the repo root, `rust-v4`, `OUT_DIR`, and
//!    `USERPROFILE`/`CARGO_HOME` so ZERO developer/local paths leak into release binaries
//!    (including `slint-build` generated code and vendored crates) without hardcoding any path.

use std::env;
use std::process::{self, Command};

fn main() {
    let mut args = env::args_os();
    let _self = args.next();
    let rustc = match args.next() {
        Some(r) => r,
        None => process::exit(1),
    };
    let rest: Vec<_> = args.collect();

    let is_probe = rest.iter().any(|a| {
        let s = a.to_string_lossy();
        s == "-vV" || s == "--version" || s.starts_with("--print")
    });

    let mut cmd = Command::new(&rustc);
    cmd.args(&rest);

    if !is_probe {
        if let Ok(exe) = env::current_exe() {
            if let Some(cargo_dir) = exe.parent() {
                let dlltool = cargo_dir.join("coff-dlltool.exe");
                if dlltool.is_file() {
                    cmd.arg("-C");
                    cmd.arg(format!("dlltool={}", dlltool.display()));
                }
                if let Some(rust_v4_dir) = cargo_dir.parent() {
                    cmd.arg("--remap-path-prefix");
                    cmd.arg(format!("{}=/src", rust_v4_dir.display()));
                    if let Some(repo_dir) = rust_v4_dir.parent() {
                        cmd.arg("--remap-path-prefix");
                        cmd.arg(format!("{}=/repo", repo_dir.display()));
                        if let Some(parent_dir) = repo_dir.parent() {
                            if parent_dir.parent().is_some() {
                                cmd.arg("--remap-path-prefix");
                                cmd.arg(format!("{}=/ws", parent_dir.display()));
                            }
                        }
                    }
                }
            }
        }
        if let Ok(up) = env::var("USERPROFILE") {
            if !up.is_empty() {
                cmd.arg("--remap-path-prefix");
                cmd.arg(format!("{up}=/user"));
            }
        }
        if let Ok(ch) = env::var("CARGO_HOME") {
            if !ch.is_empty() {
                cmd.arg("--remap-path-prefix");
                cmd.arg(format!("{ch}=/cargo"));
            }
        }
        if let Ok(od) = env::var("OUT_DIR") {
            if !od.is_empty() {
                cmd.arg("--remap-path-prefix");
                cmd.arg(format!("{od}=/out"));
            }
        }
    }

    match cmd.status() {
        Ok(st) => process::exit(st.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("rustc-wrap failed to spawn rustc: {e}");
            process::exit(1);
        }
    }
}
