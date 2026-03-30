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

/// Helper: parse the trace events JSON from the output directory.
fn load_trace_events(out_dir: &Path) -> Vec<serde_json::Value> {
    let events_path = out_dir.join("trace.bin");
    let content = std::fs::read_to_string(&events_path).expect("failed to read trace events");
    let events: serde_json::Value =
        serde_json::from_str(&content).expect("trace events should be valid JSON");
    events
        .as_array()
        .expect("events should be an array")
        .clone()
}

// ---------------------------------------------------------------------------
// Test 1: Basic execution - the tracer runs without error
// ---------------------------------------------------------------------------

#[test]
fn test_miden_tracer_basic_execution() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    // Verify the output directory and all three output files exist.
    assert!(out_dir.exists(), "output directory should exist");
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

    // trace.bin should be non-empty.
    let trace_size = std::fs::metadata(out_dir.join("trace.bin"))
        .expect("trace.bin metadata")
        .len();
    assert!(trace_size > 0, "trace.bin should be non-empty");

    // trace_metadata.json should be valid JSON with a "program" field.
    let metadata_content =
        std::fs::read_to_string(out_dir.join("trace_metadata.json")).expect("failed to read");
    let metadata: serde_json::Value =
        serde_json::from_str(&metadata_content).expect("trace_metadata.json should be valid JSON");
    assert!(
        metadata.get("program").is_some(),
        "metadata should have 'program' field"
    );

    // trace_paths.json should be a valid JSON array.
    let paths_content =
        std::fs::read_to_string(out_dir.join("trace_paths.json")).expect("failed to read");
    let paths: serde_json::Value =
        serde_json::from_str(&paths_content).expect("trace_paths.json should be valid JSON");
    assert!(paths.is_array(), "paths should be a JSON array");

    // The trace should contain Step events (i.e., execution was actually recorded).
    let events_arr = load_trace_events(&out_dir);
    let step_count = events_arr
        .iter()
        .filter(|e| e.get("Step").is_some())
        .count();
    assert!(
        step_count > 0,
        "trace should contain at least one Step event, got none"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Source mapping - step events map to correct MASM source lines
// ---------------------------------------------------------------------------

#[test]
fn test_miden_source_mapping() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    let events_arr = load_trace_events(&out_dir);

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

    // The new compute.masm has 332 lines. Steps should fall within that range.
    for step in &step_events {
        let step_rec = step.get("Step").unwrap();
        let line = step_rec["line"].as_i64().unwrap();
        assert!(
            line >= 1 && line <= 332,
            "step line should be within compute.masm range (1-332), got {}",
            line
        );
    }

    // With the comprehensive program, we should have many more step events
    // than the original simple program.
    assert!(
        step_events.len() > 20,
        "comprehensive program should produce many step events, got {}",
        step_events.len()
    );
}

// ---------------------------------------------------------------------------
// Test 3: Variable extraction - local memory slot values are captured
// ---------------------------------------------------------------------------

#[test]
fn test_miden_variable_extraction() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    let events_arr = load_trace_events(&out_dir);

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
        let val = e.get("Value").unwrap();
        val.get("value").is_some()
    });
    assert!(has_stack_var, "should have variable values");

    // Check that there are some local[N] variables (from loc_store operations).
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
    assert!(has_stack_var_name, "should have stack variable names");

    // The comprehensive program uses locals in multiple procedures
    // (fibonacci, factorial, array_sum, bitwise_ops, nested_control_flow).
    // We should see multiple different local[N] variable names.
    let local_names: std::collections::HashSet<String> = var_name_events
        .iter()
        .filter_map(|e| {
            let name = e.get("VariableName").unwrap().as_str().unwrap_or("");
            if name.starts_with("local[") {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect();
    assert!(
        local_names.len() >= 2,
        "should have at least 2 different local variable names from multiple procs, got {:?}",
        local_names
    );
}

// ---------------------------------------------------------------------------
// Test 4: Three-file output - trace.bin, trace_metadata.json, trace_paths.json
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
// Test 5: Multiple procedure calls appear in the trace
// ---------------------------------------------------------------------------

#[test]
fn test_miden_multiple_procedure_calls() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    let events_arr = load_trace_events(&out_dir);

    // There should be Function events registering procedure names.
    let function_events: Vec<&serde_json::Value> = events_arr
        .iter()
        .filter(|e| e.get("Function").is_some())
        .collect();
    assert!(
        !function_events.is_empty(),
        "there should be at least one Function event"
    );

    // Collect all function names.
    let function_names: Vec<String> = function_events
        .iter()
        .filter_map(|e| {
            let func = e.get("Function").unwrap();
            func.get("name")
                .and_then(|n| n.as_str())
                .map(|s| s.to_string())
        })
        .collect();

    // We should have multiple distinct named functions (the program has 9 procs).
    assert!(
        function_names.len() >= 2,
        "should have at least 2 registered functions, got: {:?}",
        function_names
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

    // The comprehensive program calls 10 procedures (including
    // nested_control_flow called twice), so we should see many calls.
    assert!(
        call_events.len() >= 5,
        "comprehensive program should produce at least 5 Call events, got {}",
        call_events.len()
    );
}

// ---------------------------------------------------------------------------
// Test 6: Local variable values from different procedures are captured
// ---------------------------------------------------------------------------

#[test]
fn test_miden_locals_from_multiple_procedures() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    let events_arr = load_trace_events(&out_dir);

    // Collect all Value events that have Int values (local and stack vars).
    // Format: {"Value": {"variable_id": N, "value": {"kind": "Int", "i": V, "type_id": T}}}
    let int_values: Vec<i64> = events_arr
        .iter()
        .filter_map(|e| {
            let val = e.get("Value")?;
            let value = val.get("value")?;
            if value.get("kind").and_then(|k| k.as_str()) == Some("Int") {
                value.get("i").and_then(|v| v.as_i64())
            } else {
                None
            }
        })
        .collect();

    // We should see a variety of values from different procedures:
    // - fibonacci produces values like 1, 2, 3, 5, 8, 13, 21, 34, 55
    // - factorial produces values like 7, 42, 210, 840, 2520, 5040
    // - array_sum uses addresses 100-104 and values 10, 20, 30, 40, 50
    assert!(
        int_values.len() > 50,
        "comprehensive program should produce many variable values, got {}",
        int_values.len()
    );

    // Verify some expected values appear (fibonacci sequence elements).
    assert!(
        int_values.contains(&55),
        "should see fibonacci(10) = 55 somewhere in values"
    );
}

// ---------------------------------------------------------------------------
// Test 7: Control flow through if/else branches is traced
// ---------------------------------------------------------------------------

#[test]
fn test_miden_control_flow_branches() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    let events_arr = load_trace_events(&out_dir);

    // Collect all step line numbers.
    let step_lines: Vec<i64> = events_arr
        .iter()
        .filter_map(|e| {
            let step = e.get("Step")?;
            step.get("line").and_then(|l| l.as_i64())
        })
        .collect();

    // The nested_control_flow procedure is called twice:
    // - with n=8 (takes the if.true branch at line 189)
    // - with n=3 (takes the else branch at line 202)
    //
    // We should see steps in both the if.true and else branches.
    // Lines in the if.true branch: 191-201 (repeat.3 and while loop)
    // Lines in the else branch: 204-212 (nested if)
    //
    // Verify that we have steps in the range of both branches.
    let has_if_branch_steps = step_lines.iter().any(|&l| l >= 191 && l <= 201);
    let has_else_branch_steps = step_lines.iter().any(|&l| l >= 204 && l <= 212);

    assert!(
        has_if_branch_steps,
        "should have steps in the if.true branch of nested_control_flow"
    );
    assert!(
        has_else_branch_steps,
        "should have steps in the else branch of nested_control_flow"
    );
}

// ---------------------------------------------------------------------------
// Test 8: Stack values at various points are correct
// ---------------------------------------------------------------------------

#[test]
fn test_miden_stack_values() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    let events_arr = load_trace_events(&out_dir);

    // Collect all Int values from Value events.
    // Format: {"Value": {"variable_id": N, "value": {"kind": "Int", "i": V, "type_id": T}}}
    let int_values: Vec<i64> = events_arr
        .iter()
        .filter_map(|e| {
            let val = e.get("Value")?;
            let value = val.get("value")?;
            if value.get("kind").and_then(|k| k.as_str()) == Some("Int") {
                value.get("i").and_then(|v| v.as_i64())
            } else {
                None
            }
        })
        .collect();

    // Expected values that should appear somewhere in the trace:
    // fibonacci(10) = 55
    assert!(
        int_values.contains(&55),
        "should contain fib(10) = 55"
    );

    // factorial(7) = 5040
    assert!(
        int_values.contains(&5040),
        "should contain factorial(7) = 5040"
    );

    // max_of_three(15, 42, 23): the values 15, 42, 23 should appear
    assert!(
        int_values.contains(&42),
        "should contain 42 from max_of_three"
    );

    // array_sum stores values 10, 20, 30, 40, 50 in memory
    assert!(
        int_values.contains(&10),
        "should contain 10 from array_sum"
    );

    // arithmetic_demo(100, 25): 100 + 25 = 125
    assert!(
        int_values.contains(&125),
        "should contain 125 from arithmetic_demo"
    );

    // -----------------------------------------------------------------------
    // Location-specific assertions: verify each value appears in events
    // associated with the correct procedure by tracking Call/Return events.
    // -----------------------------------------------------------------------

    // Step 1: Build a map from function_id to function name using Function events.
    // Function events are emitted with incrementing IDs starting from 0.
    let mut function_names_by_id: std::collections::HashMap<u64, String> =
        std::collections::HashMap::new();
    let mut next_function_id: u64 = 0;
    for event in &events_arr {
        if let Some(func) = event.get("Function") {
            if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                function_names_by_id.insert(next_function_id, name.to_string());
                next_function_id += 1;
            }
        }
    }

    // Step 2: Walk through events sequentially, tracking the current procedure
    // via Call/Return events, and collecting Int values per procedure.
    let mut current_proc_stack: Vec<String> = vec!["main".to_string()];
    let mut values_by_proc: std::collections::HashMap<String, Vec<i64>> =
        std::collections::HashMap::new();

    for event in &events_arr {
        if let Some(call) = event.get("Call") {
            if let Some(fn_id) = call.get("function_id").and_then(|f| f.as_u64()) {
                if let Some(name) = function_names_by_id.get(&fn_id) {
                    current_proc_stack.push(name.clone());
                }
            }
        } else if event.get("Return").is_some() {
            if current_proc_stack.len() > 1 {
                current_proc_stack.pop();
            }
        } else if let Some(val) = event.get("Value") {
            if let Some(value) = val.get("value") {
                if value.get("kind").and_then(|k| k.as_str()) == Some("Int") {
                    if let Some(int_val) = value.get("i").and_then(|v| v.as_i64()) {
                        let proc_name = current_proc_stack
                            .last()
                            .cloned()
                            .unwrap_or_else(|| "unknown".to_string());
                        values_by_proc
                            .entry(proc_name)
                            .or_default()
                            .push(int_val);
                    }
                }
            }
        }
    }

    // Helper: check if a procedure (by name suffix) produced a given value.
    let proc_has_value = |proc_suffix: &str, val: i64| -> bool {
        values_by_proc.iter().any(|(name, vals)| {
            name.ends_with(proc_suffix) && vals.contains(&val)
        })
    };

    // Verify that 55 (fib(10)) appears specifically in fibonacci procedure events.
    assert!(
        proc_has_value("fibonacci", 55),
        "value 55 should appear in fibonacci procedure events, but was not found. \
         Procedures with values: {:?}",
        values_by_proc.keys().collect::<Vec<_>>()
    );

    // Verify that 5040 (7!) appears specifically in factorial procedure events.
    assert!(
        proc_has_value("factorial", 5040),
        "value 5040 should appear in factorial procedure events, but was not found. \
         Procedures with values: {:?}",
        values_by_proc.keys().collect::<Vec<_>>()
    );

    // Verify that 42 appears specifically in max_of_three procedure events.
    assert!(
        proc_has_value("max_of_three", 42),
        "value 42 should appear in max_of_three procedure events, but was not found. \
         Procedures with values: {:?}",
        values_by_proc.keys().collect::<Vec<_>>()
    );

    // Verify that 10 appears specifically in array_sum procedure events
    // (it stores 10 as the first array element).
    assert!(
        proc_has_value("array_sum", 10),
        "value 10 should appear in array_sum procedure events, but was not found. \
         Procedures with values: {:?}",
        values_by_proc.keys().collect::<Vec<_>>()
    );

    // Verify that 125 appears specifically in arithmetic_demo procedure events.
    assert!(
        proc_has_value("arithmetic_demo", 125),
        "value 125 should appear in arithmetic_demo procedure events, but was not found. \
         Procedures with values: {:?}",
        values_by_proc.keys().collect::<Vec<_>>()
    );

    // Verify we tracked at least 5 distinct procedures that produced values,
    // confirming the procedure-level tracking is working.
    assert!(
        values_by_proc.len() >= 5,
        "should have values from at least 5 distinct procedures, got {}: {:?}",
        values_by_proc.len(),
        values_by_proc.keys().collect::<Vec<_>>()
    );
}
