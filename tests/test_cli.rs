use std::process::Command;

fn cargo_bin() -> Command {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.args(["run", "--quiet", "--"]);
    cmd
}

#[test]
fn test_help_flag() {
    let output = cargo_bin().arg("--help").output().expect("failed to run");
    assert!(output.status.success(), "--help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("codetracer-miden-recorder"),
        "help output should mention the program name"
    );
}

#[test]
fn test_version_flag() {
    let output = cargo_bin()
        .arg("--version")
        .output()
        .expect("failed to run");
    assert!(output.status.success(), "--version should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("0.1.0"),
        "version output should contain the version number"
    );
}

#[test]
fn test_record_nonexistent_file() {
    let output = cargo_bin()
        .args(["record", "nonexistent.masm"])
        .output()
        .expect("failed to run");
    assert!(
        !output.status.success(),
        "record with nonexistent file should fail"
    );
}

#[test]
fn test_record_creates_trace_files() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");

    let masm_file = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-programs/masm/compute.masm"
    );

    let output = cargo_bin()
        .args([
            "record",
            masm_file,
            "--out-dir",
            out_dir.to_str().unwrap(),
        ])
        .output()
        .expect("failed to run");

    assert!(
        output.status.success(),
        "record should succeed, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The Nim trace writer with Binary format produces a .ct container file.
    // Find *.ct files in the output directory.
    let ct_files: Vec<_> = std::fs::read_dir(&out_dir)
        .expect("failed to read output directory")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("ct") {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    assert!(
        !ct_files.is_empty(),
        "should produce at least one .ct file in output directory, found: {:?}",
        std::fs::read_dir(&out_dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect::<Vec<_>>()
    );

    // Verify the .ct file has the CTFS magic bytes: C0 DE 72 AC E2
    let ct_data = std::fs::read(&ct_files[0]).expect("failed to read .ct file");
    assert!(
        ct_data.len() >= 5,
        ".ct file should have at least 5 bytes for the magic header"
    );
    let ctfs_magic: [u8; 5] = [0xC0, 0xDE, 0x72, 0xAC, 0xE2];
    assert_eq!(
        &ct_data[..5],
        &ctfs_magic,
        ".ct file should start with CTFS magic bytes (C0 DE 72 AC E2), got {:02X?}",
        &ct_data[..5]
    );
}
