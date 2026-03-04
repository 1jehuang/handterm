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
