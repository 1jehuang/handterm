#![cfg(feature = "cli")]

use handterm::backend::Backend;
use handterm::cli::Cli;
use handterm::should_reuse_existing_host;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn print_config_matches_default_kitty_style() {
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("handterm");
    cmd.arg("print-config")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "font_family = \"JetBrainsMono Nerd Font Light\"",
        ))
        .stdout(predicate::str::contains("background = \"#000000\""))
        .stdout(predicate::str::contains("foreground = \"#cdd6f4\""))
        .stdout(predicate::str::contains("cursor = \"#f5e0dc\""));
}

#[test]
fn init_config_writes_file() {
    let temp = tempdir().expect("temp dir should be created");
    let cfg = temp.path().join("config.toml");

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("handterm");
    cmd.arg("--config")
        .arg(&cfg)
        .arg("init-config")
        .assert()
        .success()
        .stdout(predicate::str::contains("config initialized at"));

    let content = fs::read_to_string(&cfg).expect("config file should exist");
    assert!(content.contains("JetBrainsMono Nerd Font Light"));
    assert!(content.contains("background_opacity = 0.9"));
}

#[test]
fn bench_command_prints_metrics() {
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("handterm");
    cmd.arg("bench")
        .assert()
        .success()
        .stdout(predicate::str::contains("handterm benchmark results"))
        .stdout(predicate::str::contains("Theoretical Floors"))
        .stdout(predicate::str::contains("Parser"))
        .stdout(predicate::str::contains("Grid Write"))
        .stdout(predicate::str::contains("Full Terminal Pipeline"))
        .stdout(predicate::str::contains("Per-Cell Metrics"))
        .stdout(predicate::str::contains("Startup"));
}

#[test]
fn latex_command_renders_unicode_outside_handterm() {
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("handterm");
    cmd.env_remove("TERM_PROGRAM")
        .arg("latex")
        .arg(r"\frac{a}{b}")
        .assert()
        .success()
        .stdout("a\n─\nb\n");
}

#[test]
fn latex_command_emits_native_apc_inside_handterm() {
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("handterm");
    cmd.env("TERM_PROGRAM", "handterm")
        .arg("latex")
        .arg(r"\sqrt{x}")
        .assert()
        .success()
        .stdout(b"\x1b_L;\\sqrt{x}\x1b\\\n".as_slice());
}

#[test]
fn standalone_flag_bypasses_existing_host_reuse() {
    let cli = Cli {
        config: None,
        backend: Some(Backend::Gpu),
        standalone: true,
        startup_command: None,
        command: None,
    };
    assert!(!should_reuse_existing_host(&cli));

    let cli = Cli {
        config: None,
        backend: Some(Backend::Gpu),
        standalone: false,
        startup_command: None,
        command: None,
    };
    assert!(should_reuse_existing_host(&cli));
}
