//! CTFS-format audit tests for the Miden recorder.
//!
//! Added in the 1.56 audit (`AUDIT-CTFS-2026-05.md`).  Verifies the
//! canonical-CTFS pipeline checklist items closed by the audit:
//!
//!   * (a) The `record` subcommand emits the canonical CTFS multi-stream
//!     container and the resulting `.ct` file starts with the canonical
//!     magic bytes.
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
//!
//! The 2026-05-08 convention compliance follow-up tightened §4 of
//! `Recorder-CLI-Conventions.md`: recorders are now CTFS-only and
//! must not expose a `--format` flag.  Tests that previously
//! validated the `--format ctfs` value enum (specifically
//! `ctfs_format_advertised_in_record_help`) have been **deleted**:
//! they asserted on the old `--format` contract that's now inverted
//! (i.e. the help text MUST NOT contain `--format`).  Their
//! replacement coverage is at-least-as-strong: the `tests/test_cli.rs`
//! suite now contains `test_no_format_flag_in_help`,
//! `test_help_mentions_ct_print`, and
//! `test_format_flag_rejected_by_clap` which together pin the new
//! contract.  See `AUDIT-CTFS-2026-05.md`'s "Convention compliance
//! follow-up — 2026-05-08" entry for the full record (mirrors the
//! Leo recorder's commit d567b52).

use std::path::{Path, PathBuf};

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
/// Post-2026-05-08 the recorder is CTFS-only — `record` no longer takes
/// a `format` parameter.
#[test]
fn ctfs_writer_produces_ct_container() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp.path().join("ct-out");
    std::fs::create_dir_all(&out_dir).unwrap();

    codetracer_miden_recorder::recorder::record(&compute_masm_path(), &out_dir)
        .expect("recorder must produce a CTFS bundle");

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
    codetracer_miden_recorder::recorder::record(&compute_masm_path(), &out_dir)
        .expect("recorder must complete on compute.masm");

    let bytes = read_ct_container(&out_dir);
    assert!(
        bytes.len() > 64,
        ".ct container suspiciously small after call-arg staging ({} bytes)",
        bytes.len()
    );
}
