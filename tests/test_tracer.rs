//! Integration tests for the Miden tracer.

use std::path::Path;

use codetracer_trace_writer::TraceEventsFileFormat;

/// Helper: run the tracer on compute.masm and return the output directory.
fn run_tracer(out_dir: &Path) {
    let masm_path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-programs/masm/compute.masm"
    ));
    let source_code = std::fs::read_to_string(masm_path).expect("failed to read compute.masm");

    codetracer_miden_recorder::tracer::MidenTracer::trace_program(
        masm_path,
        &source_code,
        out_dir,
        TraceEventsFileFormat::Json,
    )
    .expect("trace_program should succeed");
}

// ---------------------------------------------------------------------------
// Test 1: Basic execution — the tracer runs without error
// ---------------------------------------------------------------------------

#[test]
fn test_miden_tracer_basic_execution() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    // If we got here, execution succeeded.
    assert!(out_dir.exists(), "output directory should exist");
}

// ---------------------------------------------------------------------------
// Test 2: Source mapping — step events map to correct MASM source lines
// ---------------------------------------------------------------------------

#[test]
fn test_miden_source_mapping() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    // The JSON format writes trace events to trace.bin (which is JSON in this case).
    let events_path = out_dir.join("trace.bin");
    assert!(events_path.exists(), "trace events file should exist");

    let content = std::fs::read_to_string(&events_path).expect("failed to read trace events");
    let events: serde_json::Value =
        serde_json::from_str(&content).expect("trace events should be valid JSON");
    let events_arr = events.as_array().expect("events should be an array");

    // Find Step events.
    let step_events: Vec<&serde_json::Value> = events_arr
        .iter()
        .filter(|e| e.get("Step").is_some())
        .collect();

    assert!(
        !step_events.is_empty(),
        "there should be at least one Step event"
    );

    // Verify steps have valid path_id and line fields.
    for step in &step_events {
        let step_rec = step.get("Step").unwrap();
        assert!(
            step_rec.get("path_id").is_some(),
            "Step should have path_id"
        );
        let line = step_rec["line"].as_i64().expect("line should be an integer");
        assert!(line > 0, "line number should be positive, got {}", line);
    }

    // compute.masm has content on lines 1-11. Steps should fall within that range.
    for step in &step_events {
        let step_rec = step.get("Step").unwrap();
        let line = step_rec["line"].as_i64().unwrap();
        assert!(
            line >= 1 && line <= 11,
            "step line should be within compute.masm range (1-11), got {}",
            line
        );
    }
}

// ---------------------------------------------------------------------------
// Test 3: Variable extraction — local memory slot values are captured
// ---------------------------------------------------------------------------

#[test]
fn test_miden_variable_extraction() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    let events_path = out_dir.join("trace.bin");
    let content = std::fs::read_to_string(&events_path).expect("failed to read trace events");
    let events: serde_json::Value =
        serde_json::from_str(&content).expect("trace events should be valid JSON");
    let events_arr = events.as_array().expect("events should be an array");

    // Find Value events (which contain variable values).
    let value_events: Vec<&serde_json::Value> = events_arr
        .iter()
        .filter(|e| e.get("Value").is_some())
        .collect();

    assert!(
        !value_events.is_empty(),
        "there should be at least one Value event"
    );

    // We should see stack[0] variables.
    let has_stack_var = value_events.iter().any(|e| {
        // Value events contain a FullValueRecord with variable_id
        // The variable names are registered separately; look for Int values
        let val = e.get("Value").unwrap();
        val.get("value").is_some()
    });
    assert!(has_stack_var, "should have variable values");

    // Check that there are some local[N] variables (from loc_store operations).
    // The variable names are registered via VariableName events.
    let var_name_events: Vec<&serde_json::Value> = events_arr
        .iter()
        .filter(|e| e.get("VariableName").is_some())
        .collect();

    let has_local_var = var_name_events.iter().any(|e| {
        let name = e.get("VariableName").unwrap().as_str().unwrap_or("");
        name.starts_with("local[")
    });
    assert!(
        has_local_var,
        "should have local memory slot variable names"
    );

    let has_stack_var_name = var_name_events.iter().any(|e| {
        let name = e.get("VariableName").unwrap().as_str().unwrap_or("");
        name.starts_with("stack[")
    });
    assert!(
        has_stack_var_name,
        "should have stack variable names"
    );
}

// ---------------------------------------------------------------------------
// Test 4: Three-file output — trace.bin, trace_metadata.json, trace_paths.json
// ---------------------------------------------------------------------------

#[test]
fn test_miden_trace_3file_output() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    assert!(
        out_dir.join("trace.bin").exists(),
        "trace.bin should exist"
    );
    assert!(
        out_dir.join("trace_metadata.json").exists(),
        "trace_metadata.json should exist"
    );
    assert!(
        out_dir.join("trace_paths.json").exists(),
        "trace_paths.json should exist"
    );

    // trace_metadata.json should be valid JSON with program field.
    let metadata_content =
        std::fs::read_to_string(out_dir.join("trace_metadata.json")).expect("failed to read");
    let metadata: serde_json::Value =
        serde_json::from_str(&metadata_content).expect("metadata should be valid JSON");
    assert!(
        metadata.get("program").is_some(),
        "metadata should have 'program' field"
    );

    // trace_paths.json should be a JSON array.
    let paths_content =
        std::fs::read_to_string(out_dir.join("trace_paths.json")).expect("failed to read");
    let paths: serde_json::Value =
        serde_json::from_str(&paths_content).expect("paths should be valid JSON");
    assert!(paths.is_array(), "paths should be a JSON array");
    assert!(
        !paths.as_array().unwrap().is_empty(),
        "paths should not be empty"
    );

    // trace.bin should be non-empty.
    let trace_size = std::fs::metadata(out_dir.join("trace.bin"))
        .expect("trace.bin metadata")
        .len();
    assert!(trace_size > 0, "trace.bin should be non-empty");
}

// ---------------------------------------------------------------------------
// Test 5: Procedure call trace — Call/Return events for procedure calls
// ---------------------------------------------------------------------------

#[test]
fn test_miden_procedure_call_trace() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    let events_path = out_dir.join("trace.bin");
    let content = std::fs::read_to_string(&events_path).expect("failed to read trace events");
    let events: serde_json::Value =
        serde_json::from_str(&content).expect("trace events should be valid JSON");
    let events_arr = events.as_array().expect("events should be an array");

    // There should be Function events registering procedure names.
    let function_events: Vec<&serde_json::Value> = events_arr
        .iter()
        .filter(|e| e.get("Function").is_some())
        .collect();
    assert!(
        !function_events.is_empty(),
        "there should be at least one Function event"
    );

    // In miden-processor 0.13.x, the first function is registered as the entry point.
    // The context_name doesn't distinguish between begin and exec.compute in this version.
    // Verify at least one function is registered with a name.
    let has_named_function = function_events.iter().any(|e| {
        let func = e.get("Function").unwrap();
        func.get("name")
            .and_then(|n| n.as_str())
            .is_some_and(|n| !n.is_empty())
    });
    assert!(
        has_named_function,
        "should have a Function event with a name"
    );

    // There should be Call events.
    let call_events: Vec<&serde_json::Value> = events_arr
        .iter()
        .filter(|e| e.get("Call").is_some())
        .collect();
    assert!(
        !call_events.is_empty(),
        "there should be at least one Call event"
    );

    // There should be Return events.
    let return_events: Vec<&serde_json::Value> = events_arr
        .iter()
        .filter(|e| e.get("Return").is_some())
        .collect();
    assert!(
        !return_events.is_empty(),
        "there should be at least one Return event"
    );

    // The number of calls and returns should match (balanced).
    assert_eq!(
        call_events.len(),
        return_events.len(),
        "Call and Return events should be balanced"
    );
}
