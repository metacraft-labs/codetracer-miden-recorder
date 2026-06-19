// Integration tests with explicit fixture-table literals use slice
// types that trip clippy::type_complexity.  Factoring those into
// named type aliases would make the test data harder to read in
// place.
#![allow(clippy::type_complexity)]

//! CLI-surface integration tests for `codetracer-miden-recorder`.
//!
//! Tests cover three areas:
//!
//! 1. **Smoke tests** — basic `--help`, `--version`, error paths.
//! 2. **`ct print` content** — record a fixture and pipe the resulting
//!    `.ct` container through `ct-print --json` from
//!    `codetracer-trace-format-nim` to make content-level assertions.
//!    Skips gracefully when `ct-print` is not present (i.e. when this
//!    crate is built outside the metacraft workspace).
//! 3. **CLI env-var contract** — exercise the post-2026-05-08
//!    `CODETRACER_MIDEN_RECORDER_OUT_DIR` /
//!    `CODETRACER_MIDEN_RECORDER_DISABLED` env vars and the
//!    no-`--format` invariant from `Recorder-CLI-Conventions.md` §4 / §5.
//!
//! History note: pre-2026-05-08 the recorder shipped a `--format
//! ctfs|binary|json` flag and the `test_record_creates_trace_files`
//! test passed `--format ctfs` implicitly (it was the default).  When
//! the convention switched to CTFS-only the `--format` argument was
//! removed and the smoke test was rewritten to omit it.  See
//! `AUDIT-CTFS-2026-05.md` ("Convention compliance follow-up — 2026-05-08")
//! for the full record.

use std::path::PathBuf;
use std::process::Command;

/// CTFS magic bytes: C0 DE 72 AC E2.
const CTFS_MAGIC: [u8; 5] = [0xC0, 0xDE, 0x72, 0xAC, 0xE2];

fn cargo_bin() -> Command {
    // Invoke the pre-built recorder binary directly via the
    // `CARGO_BIN_EXE_<name>` path Cargo exposes to integration tests.
    //
    // The previous `cargo run --quiet --` form spawned a *nested* `cargo`
    // inside the `cargo test` process.  The nested invocation contends for
    // the build lock on `target/` that the outer `cargo test` already
    // holds; under that contention `cargo run` can exit non-zero before it
    // ever launches the recorder, which surfaced as an intermittent
    // `--help should succeed` failure (the lock-contention window is a
    // race, so only whichever CLI test ran first was affected).  The
    // direct-binary form has no nested cargo and no lock contention.
    Command::new(env!("CARGO_BIN_EXE_codetracer-miden-recorder"))
}

fn compute_masm_path() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-programs/masm/compute.masm"
    ))
}

/// Path to the `ct-print` binary shipped with `codetracer-trace-format-nim`.
///
/// The Miden recorder is CTFS-only; tests that need to make content-level
/// assertions on a recorded trace pipe the `.ct` container through
/// `ct-print --json` and assert on the resulting JSON.  This is the
/// same workflow that `Recorder-CLI-Conventions.md` §4 prescribes for
/// downstream tools / golden snapshots.
fn ct_print_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("codetracer-trace-format-nim")
        .join(format!("ct-print{}", std::env::consts::EXE_SUFFIX))
}

// ===========================================================================
// Smoke tests
// ===========================================================================

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

    let masm_file = compute_masm_path();

    let output = cargo_bin()
        .args([
            "record",
            masm_file.to_str().unwrap(),
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

    // The Nim trace writer with CTFS produces a .ct container file.
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
        ct_data.len() >= CTFS_MAGIC.len(),
        ".ct file should have at least 5 bytes for the magic header"
    );
    assert_eq!(
        &ct_data[..CTFS_MAGIC.len()],
        &CTFS_MAGIC,
        ".ct file should start with CTFS magic bytes (C0 DE 72 AC E2), got {:02X?}",
        &ct_data[..CTFS_MAGIC.len()]
    );
}

// ===========================================================================
// CTFS content via `ct-print` — replaces the legacy `--format json` content
// assertions
// ===========================================================================

/// Record the bundled `compute.masm` fixture, then convert the
/// produced `.ct` container to JSON via `ct-print` and assert on:
///
/// 1. **Structural anchors** (legacy layer): `ct-print --json` output
///    contains the fixture source filename and at least one of the
///    MASM procedure names somewhere in the textual rendering.
/// 2. **Exact decoded values** (the layer enabled by `ct-print --full`):
///    the `compute.masm` program drives a `compute` procedure from the
///    `begin` block; `compute` then calls ten helper procedures —
///    `fibonacci(10) → 55`, `factorial(7) → 5040`,
///    `max_of_three(15, 42, 23) → 42`, `array_sum → 150`, etc.  The
///    canonical call sequence and the call-args staged by the recorder
///    (operand-stack top `s0..s3` at the call boundary) must surface
///    with decoded `Int` ValueRecords whose `i` fields match the
///    fixture's deterministic values.
///
/// Pre-2026-05-08 the recorder shipped a `--format json` mode and a
/// trace.json file was written directly.  The convention now mandates
/// CTFS-only output; `ct print` is the canonical conversion tool.  See
/// `Recorder-CLI-Conventions.md` §4.  `ct-print --full` (added 2026-05
/// in `codetracer-trace-format-nim`) is what enables the exact-value
/// layer — its output is a deterministic JSON document with every CBOR
/// `ValueRecord` decoded to a structured form like
/// `{"kind":"Int","i":42,"type_id":N}`.
#[test]
fn test_recorded_trace_via_ct_print_json() {
    let ct_print = ct_print_path();
    if !ct_print.exists() {
        eprintln!(
            "SKIP: ct-print not found at {} — only available within the \
             metacraft workspace where codetracer-trace-format-nim is a sibling.",
            ct_print.display()
        );
        return;
    }

    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    std::fs::create_dir_all(&out_dir).unwrap();

    let source_path = compute_masm_path();
    codetracer_miden_recorder::recorder::record(&source_path, &out_dir)
        .expect("recorder should succeed on compute.masm");

    let ct_files: Vec<_> = std::fs::read_dir(&out_dir)
        .expect("failed to read output directory")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "ct"))
        .collect();
    assert!(
        !ct_files.is_empty(),
        "expected at least one .ct file in {}",
        out_dir.display()
    );
    let ct_path = &ct_files[0];

    // -----------------------------------------------------------------
    // Layer 1 (legacy): ct-print --json — substring presence checks.
    // Kept as a safety net so a regression in the textual rendering
    // is caught even if --full's JSON shape evolves.
    // -----------------------------------------------------------------
    let output = Command::new(&ct_print)
        .args(["--json"])
        .arg(ct_path)
        .output()
        .expect("failed to run ct-print");

    assert!(
        output.status.success(),
        "ct-print --json should succeed; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "ct-print --json produced empty output");

    // Structural anchor 1: the fixture source path name appears in the
    // path stream rendered by ct-print.
    assert!(
        stdout.contains("compute.masm"),
        "ct-print --json output should mention the fixture source path \
         (compute.masm); got:\n{stdout}"
    );

    // Structural anchor 2: at least one of the MASM procedure names should
    // appear.  `compute.masm` declares many procedures; we look for a
    // representative subset that is unlikely to all rename at once.
    let procedure_anchor = [
        "fibonacci",
        "factorial",
        "max_of_three",
        "array_sum",
        "arithmetic_demo",
    ]
    .iter()
    .any(|v| stdout.contains(v));
    assert!(
        procedure_anchor,
        "ct-print --json output should mention at least one of the \
         MASM procedure names \
         (fibonacci/factorial/max_of_three/array_sum/arithmetic_demo); \
         got:\n{stdout}"
    );

    // -----------------------------------------------------------------
    // Layer 2 (the upgrade): ct-print --full — exact decoded values.
    // -----------------------------------------------------------------
    let full_output = Command::new(&ct_print)
        .args(["--full", "--strip-paths"])
        .arg(ct_path)
        .output()
        .expect("failed to run ct-print --full");

    assert!(
        full_output.status.success(),
        "ct-print --full should succeed; stderr: {}",
        String::from_utf8_lossy(&full_output.stderr)
    );

    let doc: serde_json::Value = serde_json::from_slice(&full_output.stdout)
        .expect("ct-print --full should emit valid JSON");

    // ----- Function table: every MASM procedure must appear -----------
    // The Miden recorder qualifies each procedure with the synthetic
    // `#exec::` prefix that the assembler attaches to procedures
    // executed from a `begin` block (the source has no explicit module
    // name).  We use `ends_with` so a future change in the prefix
    // (e.g. `module::compute::fibonacci`) does not silently break the
    // assertion.
    let functions: Vec<&str> = doc["functions"]
        .as_array()
        .expect("functions array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    for name in [
        "::fibonacci",
        "::factorial",
        "::max_of_three",
        "::array_sum",
        "::bitwise_ops",
        "::stack_manipulation",
        "::nested_control_flow",
        "::arithmetic_demo",
        "::memory_word_ops",
        "::compute",
    ] {
        assert!(
            functions.iter().any(|f| f.ends_with(name)),
            "expected a function ending with `{name}` in functions table; got {:?}",
            functions
        );
    }

    // ----- Path table: the canonical fixture path must appear ---------
    let paths: Vec<&str> = doc["paths"]
        .as_array()
        .expect("paths array")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        paths.iter().any(|p| p.ends_with("compute.masm")),
        "expected compute.masm in paths table; got {:?}",
        paths
    );

    // ----- Step / call counts ----------------------------------------
    // The Miden recorder emits one step per source-line transition for
    // each cycle that carries an AsmOp (skipping the duplicate inner
    // cycles of multi-cycle ops), plus one extra step per detected
    // backwards branch into the same source line so `repeat.N` bodies
    // surface one step per iteration (see
    // `test_control_flow_repeat_emits_step_per_iteration`).  For
    // `compute.masm` that's a stable 185 step events and 12
    // call_entry events: one synthesised `#main` for the begin-block,
    // one real `compute` call frame, and 10 helper-procedure calls
    // below `compute` (the duplicate `nested_control_flow` is invoked
    // twice).  The two trace-anchor steps (`push.0 drop`) make the
    // `#main -> compute -> helper` calltrace visible without changing
    // the operand stack.  The post-compute trace anchors keep DAP
    // step-over on distinct same-depth #main source lines after the
    // compute call returns.  These are stable properties of the canonical
    // fixture — if they change, that's a real regression to
    // investigate, not a flake.
    let counts = &doc["counts"];
    assert_eq!(
        counts["steps"].as_u64(),
        Some(185),
        "expected 185 step events for compute.masm; counts={counts}",
    );
    assert_eq!(
        counts["calls"].as_u64(),
        Some(12),
        "expected 12 call events for compute.masm; counts={counts}",
    );

    let events = doc["events"].as_array().expect("events array");

    // ----- Call sequence: every call_entry now resolves to a named
    // procedure.  The `begin` block dispatches `compute`, and
    // `compute` dispatches helpers in this exact order.  The first
    // event is the synthesised `#main` (see
    // `test_control_flow_call_exit_strict_lifo`); the second is the
    // real `compute` frame that WDIO searches for; the remaining 10
    // entries are the helper procedures observed in source order.
    let named_call_sequence: Vec<&str> = events
        .iter()
        .filter(|e| e["kind"] == "call_entry")
        .filter_map(|e| e["function"].as_str())
        .collect();
    let expected_call_sequence: &[&str] = &[
        "::#main",
        "::compute",
        "::fibonacci",
        "::factorial",
        "::max_of_three",
        "::array_sum",
        "::bitwise_ops",
        "::stack_manipulation",
        "::nested_control_flow",
        "::nested_control_flow",
        "::arithmetic_demo",
        "::memory_word_ops",
    ];
    assert_eq!(
        named_call_sequence.len(),
        expected_call_sequence.len(),
        "expected {} named call_entry events; got {:?}",
        expected_call_sequence.len(),
        named_call_sequence
    );
    for (i, expected_suffix) in expected_call_sequence.iter().enumerate() {
        assert!(
            named_call_sequence[i].ends_with(expected_suffix),
            "expected named call #{i} to end with `{expected_suffix}`; \
             got `{}` (full sequence = {:?})",
            named_call_sequence[i],
            named_call_sequence
        );
    }

    // ----- Strict ValueRecord::Int invariant on every decoded value ---
    // The Miden recorder encodes felts as `ValueRecord::Int` and
    // 4-felt Words (loaded via `mem_loadw` / `loc_loadw`) as
    // `ValueRecord::Sequence` of 4 Int felts -- see `tracer.rs`.
    // If a future change starts emitting a different variant — e.g.
    // BigInt for felts that exceed the signed-i64 range, or a typed
    // `Felt` primitive — this assertion fires loudly so the test
    // author can decide whether to extend the assertions or accept
    // the new variant.
    let mut value_count = 0usize;
    let mut check_int = |label: &str, value: &serde_json::Value| {
        assert_eq!(
            value["kind"].as_str(),
            Some("Int"),
            "{label} should decode as Int, got {value}; \
             if a new ValueRecord variant has landed for miden felts, \
             extend this test to assert on it explicitly rather than \
             weakening the check",
        );
        assert!(
            value["i"].is_i64(),
            "{label}: Int.i must be a signed integer; got {value}"
        );
        value_count += 1;
    };
    let mut check_value = |label: &str, varname: &str, value: &serde_json::Value| {
        if varname == "word" {
            // Word: 4-felt Sequence of Int.  Validate the shape
            // and recurse into the elements via check_int.
            assert_eq!(
                value["kind"].as_str(),
                Some("Sequence"),
                "{label} `word` should decode as Sequence; got {value}",
            );
            let elements = value["elements"].as_array().expect("Word.elements array");
            assert_eq!(
                elements.len(),
                4,
                "{label} `word` Sequence must have 4 elements; got {}",
                elements.len(),
            );
            for (idx, element) in elements.iter().enumerate() {
                check_int(&format!("{label} word[{idx}]"), element);
            }
        } else {
            check_int(label, value);
        }
    };
    for e in events {
        if e["kind"] == "call_entry" {
            for arg in e["args"].as_array().into_iter().flatten() {
                let name = arg["varname"].as_str().unwrap_or("?");
                check_value(&format!("call_entry arg `{name}`"), name, &arg["value"]);
            }
        } else if e["kind"] == "step" {
            for v in e["vars"].as_array().into_iter().flatten() {
                let name = v["varname"].as_str().unwrap_or("?");
                check_value(&format!("step var `{name}`"), name, &v["value"]);
            }
        }
    }
    assert!(
        value_count > 0,
        "expected at least one decoded ValueRecord in events; got 0"
    );

    // ----- Exact (varname, value) assertions on canonical call args --
    // The Miden recorder stages the visible operand-stack top
    // (`s0..s3`) at every call boundary as canonical args (see
    // `tracer.rs::process_vm_states`'s call-detection block).  The
    // `compute.masm` fixture is fully deterministic — every call
    // boundary produces exact, fixed felt values for `s0..s3`:
    //
    //   * compute entry: stack-neutral trace anchors leave s0..s3 = 0
    //   * fibonacci entry: stack carries the input `n=10`     → s1=10
    //   * factorial entry: stack still has the fib(10)=55     → s2=55,
    //     and factorial's input `n=7` is on top                → s1=7
    //   * max_of_three entry: largest of (15,42,23) reduction → s2=42
    //   * array_sum entry: carries factorial(7)=5040          → s2=5040
    //   * nested_control_flow entry: input n=8                → s0=8
    //
    // These tie directly to the source program's `compute` body.  Any
    // change that breaks them (a renamed procedure, a different stack
    // discipline at the call boundary, or a different felt encoding)
    // is a real regression worth investigating.
    fn call_args(
        events: &[serde_json::Value],
        suffix: &str,
        occurrence: usize,
    ) -> Vec<(String, i64)> {
        events
            .iter()
            .filter(|e| e["kind"] == "call_entry")
            .filter(|e| e["function"].as_str().is_some_and(|f| f.ends_with(suffix)))
            .nth(occurrence)
            .unwrap_or_else(|| {
                panic!("could not find call_entry #{occurrence} ending with `{suffix}`")
            })["args"]
            .as_array()
            .expect("args array")
            .iter()
            .map(|a| {
                let name = a["varname"].as_str().unwrap().to_string();
                let i = a["value"]["i"]
                    .as_i64()
                    .unwrap_or_else(|| panic!("arg `{name}` Int.i not i64"));
                (name, i)
            })
            .collect()
    }

    let expected_args: &[(&str, usize, &[(&str, i64)])] = &[
        (
            "::compute",
            0,
            &[("s0", 0), ("s1", 0), ("s2", 0), ("s3", 0)],
        ),
        (
            "::fibonacci",
            0,
            &[("s0", 0), ("s1", 10), ("s2", 0), ("s3", 0)],
        ),
        (
            "::factorial",
            0,
            &[("s0", 0), ("s1", 7), ("s2", 55), ("s3", 0)],
        ),
        (
            "::max_of_three",
            0,
            &[("s0", 42), ("s1", 23), ("s2", 42), ("s3", 15)],
        ),
        (
            "::array_sum",
            0,
            &[("s0", 100), ("s1", 15), ("s2", 5040), ("s3", 55)],
        ),
        // The fixture intentionally calls nested_control_flow twice,
        // once with n=8 and once with n=3. Pin both call boundaries so
        // a missing/reordered helper frame is caught by decoded args,
        // not just by the call sequence assertion above.
        (
            "::nested_control_flow",
            0,
            &[("s0", 0), ("s1", 8), ("s2", 150), ("s3", 15)],
        ),
        (
            "::nested_control_flow",
            1,
            &[("s0", 0), ("s1", 3), ("s2", 9), ("s3", 150)],
        ),
    ];
    for (suffix, occurrence, expected) in expected_args {
        let observed = call_args(events, suffix, *occurrence);
        assert_eq!(
            observed.len(),
            expected.len(),
            "call_entry #{occurrence} ending with `{suffix}` should have \
             {} args; got {:?}",
            expected.len(),
            observed
        );
        for ((on, ov), (en, ev)) in observed.iter().zip(expected.iter()) {
            assert_eq!(
                on, en,
                "call_entry #{occurrence} ending with `{suffix}`: \
                 expected arg name `{en}` at this position; got `{on}` \
                 (full args = {:?})",
                observed
            );
            assert_eq!(
                ov, ev,
                "call_entry #{occurrence} ending with `{suffix}`: arg \
                 `{en}` should be {ev}; got {ov} (full args = {:?})",
                observed
            );
        }
    }

    // ----- Exact step-variable assertion: fib computes 55 -------------
    // The call boundary above pins the input (`s1 = 10`).  Inside
    // `#exec::fibonacci`, the decoded step variables must also surface
    // the computed result `stack[0] = 55`.
    let fib_has_stack0_55 = events
        .iter()
        .filter(|e| {
            e["kind"] == "step"
                && e["function"]
                    .as_str()
                    .is_some_and(|f| f.ends_with("::fibonacci"))
        })
        .flat_map(|e| e["vars"].as_array().into_iter().flatten())
        .any(|v| v["varname"] == "stack[0]" && v["value"]["i"].as_i64() == Some(55));
    assert!(
        fib_has_stack0_55,
        "fibonacci frame should surface computed stack[0] = 55"
    );
}

// ===========================================================================
// CLI env-var contract
// ===========================================================================

/// `CODETRACER_MIDEN_RECORDER_OUT_DIR` must be honoured as a fallback
/// for `--out-dir`.  Convention: `Recorder-CLI-Conventions.md` §5.
#[test]
fn test_env_out_dir_used_when_flag_omitted() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let env_out_dir = tmp_dir.path().join("via-env");

    let source_path = compute_masm_path();

    let output = Command::new(env!("CARGO_BIN_EXE_codetracer-miden-recorder"))
        .args(["record"])
        .arg(&source_path)
        .env("CODETRACER_MIDEN_RECORDER_OUT_DIR", &env_out_dir)
        // Make sure the env-var doesn't bleed in from the developer's shell.
        .env_remove("CODETRACER_MIDEN_RECORDER_DISABLED")
        .output()
        .expect("failed to run recorder");

    assert!(
        output.status.success(),
        "recorder should succeed when CODETRACER_MIDEN_RECORDER_OUT_DIR is set; \
         stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The env-var-supplied output dir must contain the .ct bundle.
    let ct_files: Vec<_> = std::fs::read_dir(&env_out_dir)
        .unwrap_or_else(|e| {
            panic!(
                "expected env-supplied out-dir {:?} to exist after record: {e}",
                env_out_dir
            )
        })
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "ct"))
        .collect();
    assert!(
        !ct_files.is_empty(),
        "expected the env-supplied output dir {:?} to receive the .ct trace bundle",
        env_out_dir
    );
}

/// `CODETRACER_MIDEN_RECORDER_DISABLED=1` must skip recording entirely.
/// The recorder process should still exit 0 (the Miden recorder doesn't
/// run a separate target subprocess — it assembles & executes the
/// source itself — so "disabled" simply means "don't write any
/// trace artefacts").
#[test]
fn test_env_disabled_skips_recording() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp_dir.path().join("should-stay-empty");

    let source_path = compute_masm_path();

    let output = Command::new(env!("CARGO_BIN_EXE_codetracer-miden-recorder"))
        .args(["record"])
        .arg(&source_path)
        .args(["--out-dir"])
        .arg(&out_dir)
        .env("CODETRACER_MIDEN_RECORDER_DISABLED", "1")
        .output()
        .expect("failed to run recorder");

    assert!(
        output.status.success(),
        "recorder should succeed in disabled mode; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // No trace artefacts of any kind should have been written.
    let no_artefacts = !out_dir.exists()
        || (std::fs::read_dir(&out_dir)
            .map(|rd| rd.filter_map(|e| e.ok()).next().is_none())
            .unwrap_or(true));
    assert!(
        no_artefacts,
        "no trace artefacts should be written when \
         CODETRACER_MIDEN_RECORDER_DISABLED=1; got files in {:?}",
        out_dir
    );
}

/// `--format` is no longer accepted at any level — clap must reject it.
/// Convention: §4 (CTFS-only).
#[test]
fn test_format_flag_rejected_by_clap() {
    let tmp_dir = tempfile::tempdir().expect("tempdir");
    let out_dir = tmp_dir.path().join("traces");
    let source_path = compute_masm_path();

    let output = Command::new(env!("CARGO_BIN_EXE_codetracer-miden-recorder"))
        .args(["record"])
        .arg(&source_path)
        .args(["--out-dir"])
        .arg(&out_dir)
        .args(["--format", "json"])
        .output()
        .expect("failed to run recorder");

    assert!(
        !output.status.success(),
        "--format should be rejected by clap; stdout: {}, stderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--format")
            || stderr.contains("unexpected argument")
            || stderr.contains("unrecognized")
            || stderr.contains("found argument"),
        "clap error should mention the unknown --format flag; got stderr:\n{stderr}"
    );
}

/// The CLI binary must not expose a `--format` flag at any level.
/// Convention: `Recorder-CLI-Conventions.md` §4 — recorders are
/// CTFS-only.
#[test]
fn test_no_format_flag_in_help() {
    let bin = env!("CARGO_BIN_EXE_codetracer-miden-recorder");

    for subcmd in [None, Some("record"), Some("contract"), Some("replay")] {
        let mut cmd = Command::new(bin);
        if let Some(s) = subcmd {
            cmd.arg(s);
        }
        cmd.arg("--help");

        let output = cmd.output().expect("failed to run --help");
        assert!(
            output.status.success(),
            "--help (subcmd={:?}) should exit 0",
            subcmd
        );

        let help = String::from_utf8_lossy(&output.stdout);
        assert!(
            !help.contains("--format"),
            "--help (subcmd={:?}) must not advertise --format; got:\n{help}",
            subcmd
        );
        assert!(
            !help.contains("CODETRACER_FORMAT"),
            "--help (subcmd={:?}) must not advertise CODETRACER_FORMAT; got:\n{help}",
            subcmd
        );
    }
}

/// `--help` must mention `ct print` so users know where to go for
/// human-readable conversion of the recorded CTFS bundle.
#[test]
fn test_help_mentions_ct_print() {
    let bin = env!("CARGO_BIN_EXE_codetracer-miden-recorder");
    let output = Command::new(bin)
        .arg("--help")
        .output()
        .expect("failed to run --help");
    assert!(output.status.success(), "--help should exit 0");

    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.contains("ct print"),
        "--help must mention `ct print` as the conversion tool; got:\n{help}"
    );
}
