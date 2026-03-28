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

    assert!(
        out_dir.join("trace_metadata.json").exists(),
        "trace_metadata.json should exist in output directory"
    );
    assert!(
        out_dir.join("trace_paths.json").exists(),
        "trace_paths.json should exist in output directory"
    );

    // Verify trace_metadata.json is valid JSON
    let metadata_content =
        std::fs::read_to_string(out_dir.join("trace_metadata.json")).expect("failed to read metadata");
    let metadata: serde_json::Value =
        serde_json::from_str(&metadata_content).expect("trace_metadata.json should be valid JSON");
    assert_eq!(
        metadata["recorder"], "codetracer-miden-recorder",
        "metadata should identify the recorder"
    );
}
