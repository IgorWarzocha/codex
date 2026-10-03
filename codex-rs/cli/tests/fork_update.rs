use std::process::Command;

#[test]
fn fork_update_preserves_installation_and_configuration() -> anyhow::Result<()> {
    let codex = codex_utils_cargo_bin::cargo_bin("codex")?;
    let home = tempfile::tempdir()?;
    let config = home.path().join("config.toml");
    let installed = home.path().join("packages/standalone/current/bin/codex");
    std::fs::create_dir_all(installed.parent().expect("installation directory"))?;
    std::fs::write(&installed, b"installed fork sentinel")?;
    std::fs::write(&config, b"# preserve user configuration\n")?;

    let output = Command::new(codex)
        .arg("update")
        .current_dir(home.path())
        .env("CODEX_HOME", home.path())
        .env("PATH", "")
        .output()?;

    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr)?;
    assert!(error.contains("Update Codex Lean manually"), "{error}");
    assert!(error.contains("https://github.com/IgorWarzocha/codex-lean/releases"));
    assert_eq!(std::fs::read(&installed)?, b"installed fork sentinel");
    assert_eq!(std::fs::read(&config)?, b"# preserve user configuration\n");
    assert!(!home.path().join("version.json").exists());
    Ok(())
}
