//! CTFS-format audit tests for the Miden recorder.
//!
//! Added in the 1.56 audit (`AUDIT-CTFS-2026-05.md`).  Verifies the
//! canonical-CTFS pipeline checklist items closed by the audit:
//!
//!   * (a) The `record` subcommand defaults to the CTFS multi-stream
//!     container and the resulting `.ct` file starts with the canonical
//!     magic bytes.
//!   * (a) The CLI advertises `ctfs` as a `--format` value with
//!     `[default: ctfs]` (catches accidental defaults regressions).
//!   * (c) Staging the operand-stack top through `TraceWriter::arg`
//!     (so the call-arg channel is materially populated) does not
//!     regress the size or magic of the `.ct` container; the canonical
//!     writer still produces a populated trace when a real MASM
//!     program with `exec.<proc>` calls is recorded.
//!
//! Pre-fix the recorder had no way to request CTFS at all (only the
//! legacy `Binary` and `Json` variants of `OutputFormat`), did not
//! stage call arguments through `register_call_arg`/`writer.arg`, and
//! did not route VM execution errors through `register_special_event`.

use std::path::{Path, PathBuf};
use std::process::Command;

use codetracer_trace_writer_nim::TraceEventsFileFormat;

/// Canonical CTFS container magic bytes.  Mirrors the constant
/// `CTFS_MAGIC` in `codetracer-trace-format-spec/src/container.rs`.
const CTFS_MAGIC: [u8; 5] = [0xC0, 0xDE, 0x72, 0xAC, 0xE2];

/// Path to the bundled `compute.masm` test program, which exercises
/// procedures, locals, control flow, memory, and arithmetic — enough
/// to materially populate every CTFS stream the recorder writes.
fn compute_masm_path() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-programs/masm/compute.masm"
    ))
}

/// Find the single `.ct` file in `out_dir`, asserting that exactly one
/// exists and that it begins with the canonical CTFS magic bytes.
fn read_ct_container(out_dir: &Path) -> Vec<u8> {
    let entries: Vec<_> = std::fs::read_dir(out_dir)
        .expect("read output directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("ct"))
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one .ct file in {:?}, got {:?}",
        out_dir,
        entries
    );
    let bytes = std::fs::read(&entries[0]).expect("read .ct file");
    assert!(
        bytes.len() >= CTFS_MAGIC.len(),
        ".ct file too short to contain CTFS magic"
    );
    assert_eq!(
        &bytes[..CTFS_MAGIC.len()],
        &CTFS_MAGIC,
        ".ct file does not start with canonical CTFS magic bytes"
    );
    bytes
}

/// Audit (a)+(g): writing through the canonical CTFS pipeline produces a
/// `.ct` container with the canonical magic header.  Pre-fix the recorder
/// had no way to request CTFS at all (only legacy `Binary` / `Json`).
#[test]
fn ctfs_writer_produces_ct_container() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp.path().join("ct-out");
    std::fs::create_dir_all(&out_dir).unwrap();

    codetracer_miden_recorder::recorder::record(
        &compute_masm_path(),
        &out_dir,
        TraceEventsFileFormat::Ctfs,
    )
    .expect("recorder must accept TraceEventsFileFormat::Ctfs");

    let bytes = read_ct_container(&out_dir);
    // A materially populated trace contains far more than just the magic
    // header (program metadata, type id table, register-name table, step
    // events, call/return events).  64 bytes is a loose lower bound that
    // catches a regression where the audit fix path silently empties the
    // event stream.
    assert!(
        bytes.len() > 64,
        ".ct container suspiciously small ({} bytes)",
        bytes.len()
    );
}

/// Audit (a): `record --help` advertises `ctfs` as a `--format` value
/// with `[default: ctfs]`.  Catches accidental regressions to the legacy
/// `Binary` default.
#[test]
fn ctfs_format_advertised_in_record_help() {
    let bin = env!("CARGO_BIN_EXE_codetracer-miden-recorder");
    let output = Command::new(bin)
        .args(["record", "--help"])
        .output()
        .expect("running record --help");
    assert!(output.status.success(), "record --help should succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("ctfs"),
        "record --help should list ctfs as a --format value, got:\n{stdout}"
    );
    assert!(
        stdout.contains("[default: ctfs]"),
        "record --help should advertise ctfs as the default format, got:\n{stdout}"
    );
}

/// Audit (c): staging the operand-stack top through `TraceWriter::arg`
/// at every detected procedure entry must not empty the trace.  Pre-fix
/// the recorder did not stage any call arguments at all; this test
/// asserts the post-fix path still produces a canonical-CTFS container
/// with content when a real MASM program with multiple `exec.<proc>`
/// call sites is recorded.
#[test]
fn call_arg_staging_does_not_empty_trace() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp.path().join("ct-out");
    std::fs::create_dir_all(&out_dir).unwrap();

    // compute.masm contains 10+ `exec.<proc>` call sites, each of which
    // exercises the new stack[0..3] -> arg("s0..s3") staging path added
    // in the 1.56 audit.
    codetracer_miden_recorder::recorder::record(
        &compute_masm_path(),
        &out_dir,
        TraceEventsFileFormat::Ctfs,
    )
    .expect("recorder must complete on compute.masm");

    let bytes = read_ct_container(&out_dir);
    assert!(
        bytes.len() > 64,
        ".ct container suspiciously small after call-arg staging ({} bytes)",
        bytes.len()
    );
}
