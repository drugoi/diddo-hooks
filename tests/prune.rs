#[test]
fn prune_help_lists_retention_and_preview_options() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_diddo"))
        .args(["prune", "--help"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for flag in ["--days", "--dry-run", "--yes"] {
        assert!(help.contains(flag), "{help}");
    }
}
