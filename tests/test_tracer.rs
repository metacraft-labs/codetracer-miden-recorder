//! Integration tests for the Miden tracer.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use codetracer_trace_writer_nim::TraceEventsFileFormat;

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
    let events_path = out_dir.join("trace.json");
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
        out_dir.join("trace.json").exists(),
        "trace.json should exist"
    );
    assert!(
        out_dir.join("trace_metadata.json").exists(),
        "trace_metadata.json should exist"
    );
    assert!(
        out_dir.join("trace_paths.json").exists(),
        "trace_paths.json should exist"
    );

    // trace.json should be non-empty.
    let trace_size = std::fs::metadata(out_dir.join("trace.json"))
        .expect("trace.json metadata")
        .len();
    assert!(trace_size > 0, "trace.json should be non-empty");

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

    // Derive the total line count from the source file at runtime
    // instead of hardcoding it.
    let masm_path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-programs/masm/compute.masm"
    ));
    let source_code = std::fs::read_to_string(masm_path).expect("failed to read compute.masm");
    let total_lines = source_code.lines().count() as i64;

    for step in &step_events {
        let step_rec = step.get("Step").unwrap();
        let line = step_rec["line"].as_i64().unwrap();
        assert!(
            line >= 1 && line <= total_lines,
            "step line should be within compute.masm range (1-{}), got {}",
            total_lines,
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
// Test 4: Three-file output - trace.json, trace_metadata.json, trace_paths.json
// ---------------------------------------------------------------------------

#[test]
fn test_miden_trace_3file_output() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    assert!(
        out_dir.join("trace.json").exists(),
        "trace.json should exist"
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

    // trace.json should be non-empty.
    let trace_size = std::fs::metadata(out_dir.join("trace.json"))
        .expect("trace.json metadata")
        .len();
    assert!(trace_size > 0, "trace.json should be non-empty");
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

    // Derive the if.true and else branch line ranges from compute.masm at runtime
    // instead of hardcoding them. We find the nested_control_flow procedure's
    // if.true and else keywords and determine ranges from there.
    let masm_path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-programs/masm/compute.masm"
    ));
    let source_code = std::fs::read_to_string(masm_path).expect("failed to read compute.masm");
    let source_lines: Vec<&str> = source_code.lines().collect();

    // Find the nested_control_flow procedure boundaries.
    let proc_start = source_lines
        .iter()
        .position(|l| l.trim().starts_with("proc.nested_control_flow"))
        .expect("should find proc.nested_control_flow");
    // Find the end of this procedure (next "end" at the procedure level).
    let proc_end = source_lines[proc_start..]
        .iter()
        .enumerate()
        .filter(|(i, l)| *i > 0 && l.trim() == "end")
        .last()
        .map(|(i, _)| proc_start + i)
        .expect("should find end of nested_control_flow");

    // Within the procedure, find the first if.true and else keywords.
    let if_true_offset = source_lines[proc_start..=proc_end]
        .iter()
        .position(|l| l.trim() == "if.true")
        .expect("should find if.true in nested_control_flow");
    let if_true_line = proc_start + if_true_offset; // 0-indexed

    let else_offset = source_lines[proc_start..=proc_end]
        .iter()
        .position(|l| l.trim() == "else")
        .expect("should find else in nested_control_flow");
    let else_line = proc_start + else_offset; // 0-indexed

    // Find the "end" that closes the if/else block (first "end" after the else).
    let if_end_offset = source_lines[(proc_start + else_offset)..]
        .iter()
        .position(|l| l.trim() == "end")
        .expect("should find end after else");
    let if_end_line = proc_start + else_offset + if_end_offset; // 0-indexed

    // Convert to 1-indexed source lines for comparison with step events.
    // if.true branch body: from line after if.true to line before else
    let if_branch_start = (if_true_line + 2) as i64; // 1-indexed, skip the if.true line itself
    let if_branch_end = else_line as i64;             // 1-indexed (the else line, exclusive)
    // else branch body: from line after else to line before its end
    let else_branch_start = (else_line + 2) as i64;   // 1-indexed, skip the else line itself
    let else_branch_end = if_end_line as i64;          // 1-indexed

    let has_if_branch_steps = step_lines
        .iter()
        .any(|&l| l >= if_branch_start && l <= if_branch_end);
    let has_else_branch_steps = step_lines
        .iter()
        .any(|&l| l >= else_branch_start && l <= else_branch_end);

    assert!(
        has_if_branch_steps,
        "should have steps in the if.true branch of nested_control_flow (lines {}-{})",
        if_branch_start, if_branch_end
    );
    assert!(
        has_else_branch_steps,
        "should have steps in the else branch of nested_control_flow (lines {}-{})",
        else_branch_start, else_branch_end
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

// ===========================================================================
// Shared helpers for procedure-scoped event tracking
// ===========================================================================

/// Build a map from function_id to function name from Function events.
fn build_function_id_map(events: &[serde_json::Value]) -> HashMap<u64, String> {
    let mut map = HashMap::new();
    let mut next_id: u64 = 0;
    for event in events {
        if let Some(func) = event.get("Function") {
            if let Some(name) = func.get("name").and_then(|n| n.as_str()) {
                map.insert(next_id, name.to_string());
                next_id += 1;
            }
        }
    }
    map
}

/// Walk events tracking the current procedure via Call/Return and collect
/// per-procedure data using a user-supplied callback.
///
/// The callback receives (current_proc_name, event) for every event.
fn walk_events_with_proc_context<F>(events: &[serde_json::Value], mut callback: F)
where
    F: FnMut(&str, &serde_json::Value),
{
    let fn_map = build_function_id_map(events);
    let mut proc_stack: Vec<String> = vec!["main".to_string()];

    for event in events {
        if let Some(call) = event.get("Call") {
            if let Some(fn_id) = call.get("function_id").and_then(|f| f.as_u64()) {
                if let Some(name) = fn_map.get(&fn_id) {
                    proc_stack.push(name.clone());
                }
            }
        } else if event.get("Return").is_some() {
            if proc_stack.len() > 1 {
                proc_stack.pop();
            }
        }

        let current = proc_stack.last().map(|s| s.as_str()).unwrap_or("unknown");
        callback(current, event);
    }
}

/// Read the compute.masm source and return it as a String.
fn read_compute_masm() -> String {
    let masm_path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-programs/masm/compute.masm"
    ));
    std::fs::read_to_string(masm_path).expect("failed to read compute.masm")
}

// ---------------------------------------------------------------------------
// Test 9: Fibonacci value at location — verify local[2] never exists (fibonacci
// uses local[0] and local[1]), and that the value 55 appears as a variable
// value specifically within the fibonacci procedure context.
// ---------------------------------------------------------------------------

#[test]
fn test_miden_fibonacci_value_at_location() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    let events_arr = load_trace_events(&out_dir);
    let source = read_compute_masm();

    // Find the fibonacci procedure line range (1-indexed).
    let source_lines: Vec<&str> = source.lines().collect();
    let fib_start = source_lines
        .iter()
        .position(|l| l.trim().starts_with("proc.fibonacci"))
        .expect("should find proc.fibonacci") + 1; // 1-indexed
    let fib_body_end = source_lines[fib_start..]
        .iter()
        .position(|l| l.trim() == "end")
        .expect("should find end of fibonacci")
        + fib_start
        + 1; // 1-indexed

    // Collect Step events that fall within the fibonacci procedure lines.
    let fib_step_lines: Vec<i64> = events_arr
        .iter()
        .filter_map(|e| {
            let step = e.get("Step")?;
            let line = step.get("line")?.as_i64()?;
            if line >= fib_start as i64 && line <= fib_body_end as i64 {
                Some(line)
            } else {
                None
            }
        })
        .collect();

    assert!(
        !fib_step_lines.is_empty(),
        "should have Step events within the fibonacci procedure (lines {}-{})",
        fib_start, fib_body_end
    );

    // Track variable names and values per procedure using Call/Return context.
    // We want to verify that within the fibonacci procedure context:
    // 1. local[1] (current) eventually holds 55
    // 2. Step events are emitted at correct source lines

    let mut fib_local_values: HashMap<String, Vec<i64>> = HashMap::new();
    let mut fib_step_count = 0usize;

    walk_events_with_proc_context(&events_arr, |proc_name, event| {
        if !proc_name.ends_with("fibonacci") {
            return;
        }

        if event.get("Step").is_some() {
            fib_step_count += 1;
        }

        // Track VariableName -> variable_id mapping.
        // Then match Value events by variable_id.
        if let Some(val) = event.get("Value") {
            if let Some(value) = val.get("value") {
                if value.get("kind").and_then(|k| k.as_str()) == Some("Int") {
                    if let Some(int_val) = value.get("i").and_then(|v| v.as_i64()) {
                        // Get the variable_id to correlate with the most recent VariableName.
                        if let Some(var_id) = val.get("variable_id").and_then(|v| v.as_u64()) {
                            fib_local_values
                                .entry(format!("var_{}", var_id))
                                .or_default()
                                .push(int_val);
                        }
                    }
                }
            }
        }
    });

    assert!(
        fib_step_count > 0,
        "fibonacci procedure should produce Step events within its context"
    );

    // Verify that 55 appears as a value within the fibonacci procedure context.
    let all_fib_values: Vec<i64> = fib_local_values.values().flatten().copied().collect();
    assert!(
        all_fib_values.contains(&55),
        "fibonacci procedure should produce value 55 (fib(10)), got values: {:?}",
        all_fib_values
    );

    // Verify the fibonacci sequence building: we should see intermediate values
    // from the sequence (1, 1, 2, 3, 5, 8, 13, 21, 34, 55).
    let fib_sequence = [1i64, 2, 3, 5, 8, 13, 21, 34, 55];
    let found_fib_values: Vec<i64> = fib_sequence
        .iter()
        .filter(|v| all_fib_values.contains(v))
        .copied()
        .collect();
    assert!(
        found_fib_values.len() >= 3,
        "should see at least 3 fibonacci sequence values within fibonacci context, \
         found: {:?}",
        found_fib_values
    );
}

// ---------------------------------------------------------------------------
// Test 10: Call tree structure — verify Function/Call/Return events match
// the expected program structure: begin -> exec.compute procedures
// ---------------------------------------------------------------------------

#[test]
fn test_miden_call_tree_structure() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    let events_arr = load_trace_events(&out_dir);

    // Build the function_id -> name map.
    let fn_map = build_function_id_map(&events_arr);

    // Verify all expected procedures are registered as Function events.
    let registered_names: HashSet<String> = fn_map.values().cloned().collect();
    let expected_procs = [
        "fibonacci",
        "factorial",
        "max_of_three",
        "array_sum",
        "bitwise_ops",
        "stack_manipulation",
        "nested_control_flow",
        "arithmetic_demo",
        "memory_word_ops",
    ];
    for proc_name in &expected_procs {
        assert!(
            registered_names.iter().any(|n| n.ends_with(proc_name)),
            "Function event should be registered for '{}', registered: {:?}",
            proc_name,
            registered_names
        );
    }

    // Walk the events and reconstruct the call tree as a sequence of
    // (event_type, proc_name) tuples.
    let mut call_tree: Vec<(String, String)> = Vec::new();
    for event in &events_arr {
        if let Some(call) = event.get("Call") {
            if let Some(fn_id) = call.get("function_id").and_then(|f| f.as_u64()) {
                if let Some(name) = fn_map.get(&fn_id) {
                    call_tree.push(("Call".to_string(), name.clone()));
                }
            }
        } else if event.get("Return").is_some() {
            call_tree.push(("Return".to_string(), String::new()));
        }
    }

    // Verify balanced Call/Return pairs.
    let call_count = call_tree.iter().filter(|(t, _)| t == "Call").count();
    let return_count = call_tree.iter().filter(|(t, _)| t == "Return").count();
    assert_eq!(
        call_count, return_count,
        "Call and Return events must be balanced: {} calls vs {} returns",
        call_count, return_count
    );

    // Verify the call order matches the program structure in main (begin block).
    // The program calls procedures in this order:
    // fibonacci, factorial, max_of_three, array_sum, bitwise_ops,
    // stack_manipulation, nested_control_flow, nested_control_flow,
    // arithmetic_demo, memory_word_ops
    let call_order: Vec<String> = call_tree
        .iter()
        .filter(|(t, _)| t == "Call")
        .map(|(_, name)| {
            // Extract the short name (last segment after ::).
            name.rsplit("::").next().unwrap_or(name).to_string()
        })
        .collect();

    let expected_order = [
        "fibonacci",
        "factorial",
        "max_of_three",
        "array_sum",
        "bitwise_ops",
        "stack_manipulation",
        "nested_control_flow",
        "nested_control_flow",
        "arithmetic_demo",
        "memory_word_ops",
    ];

    // Verify the calls appear in the expected order.
    // There may be additional framework calls, so we check subsequence matching.
    let mut order_idx = 0;
    for call_name in &call_order {
        if order_idx < expected_order.len() && call_name == expected_order[order_idx] {
            order_idx += 1;
        }
    }
    assert_eq!(
        order_idx,
        expected_order.len(),
        "call order should match expected program structure. \
         Expected {:?}, got calls: {:?}",
        expected_order,
        call_order
    );

    // Verify that Call/Return events are properly nested: at no point should the
    // depth go below zero.
    let mut depth: i32 = 0;
    for (event_type, _) in &call_tree {
        match event_type.as_str() {
            "Call" => depth += 1,
            "Return" => {
                depth -= 1;
                assert!(
                    depth >= 0,
                    "Return event without matching Call (depth went negative)"
                );
            }
            _ => {}
        }
    }
    assert_eq!(
        depth, 0,
        "call stack should be empty at end of trace, depth = {}",
        depth
    );
}

// ---------------------------------------------------------------------------
// Test 11: Conditional branch coverage — verify both if.true and else branches
// in nested_control_flow produce Step events at specific branch lines
// ---------------------------------------------------------------------------

#[test]
fn test_miden_conditional_branch_coverage() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    let events_arr = load_trace_events(&out_dir);
    let source = read_compute_masm();
    let source_lines: Vec<&str> = source.lines().collect();

    // Find the nested_control_flow procedure.
    let proc_start = source_lines
        .iter()
        .position(|l| l.trim().starts_with("proc.nested_control_flow"))
        .expect("should find proc.nested_control_flow");

    // Find the first if.true within the procedure.
    let if_true_idx = source_lines[proc_start..]
        .iter()
        .position(|l| l.trim() == "if.true")
        .expect("should find if.true")
        + proc_start;

    // Find the else keyword.
    let else_idx = source_lines[if_true_idx..]
        .iter()
        .position(|l| l.trim() == "else")
        .expect("should find else")
        + if_true_idx;

    // Find the end that closes the if/else.
    let if_end_idx = source_lines[else_idx..]
        .iter()
        .position(|l| l.trim() == "end")
        .expect("should find end after else")
        + else_idx;

    // Identify specific lines in each branch that contain executable operations.
    // if.true branch: lines between if_true_idx and else_idx (exclusive, 0-indexed)
    let if_branch_executable_lines: Vec<usize> = (if_true_idx + 1..else_idx)
        .filter(|&i| {
            let trimmed = source_lines[i].trim();
            !trimmed.is_empty()
                && !trimmed.starts_with('#')
                && trimmed != "end"
                && !trimmed.starts_with("repeat")
                && !trimmed.starts_with("while")
        })
        .collect();

    // else branch: lines between else_idx and if_end_idx (exclusive, 0-indexed)
    let else_branch_executable_lines: Vec<usize> = (else_idx + 1..if_end_idx)
        .filter(|&i| {
            let trimmed = source_lines[i].trim();
            !trimmed.is_empty()
                && !trimmed.starts_with('#')
                && trimmed != "end"
                && !trimmed.starts_with("if.")
                && !trimmed.starts_with("else")
        })
        .collect();

    assert!(
        !if_branch_executable_lines.is_empty(),
        "should find executable lines in if.true branch"
    );
    assert!(
        !else_branch_executable_lines.is_empty(),
        "should find executable lines in else branch"
    );

    // Collect step lines that occur within the nested_control_flow procedure context.
    let mut ncf_step_lines: Vec<i64> = Vec::new();
    walk_events_with_proc_context(&events_arr, |proc_name, event| {
        if !proc_name.ends_with("nested_control_flow") {
            return;
        }
        if let Some(step) = event.get("Step") {
            if let Some(line) = step.get("line").and_then(|l| l.as_i64()) {
                ncf_step_lines.push(line);
            }
        }
    });

    // Convert 0-indexed to 1-indexed for comparison with step events.
    let if_branch_lines_1indexed: Vec<i64> = if_branch_executable_lines
        .iter()
        .map(|&i| (i + 1) as i64)
        .collect();
    let else_branch_lines_1indexed: Vec<i64> = else_branch_executable_lines
        .iter()
        .map(|&i| (i + 1) as i64)
        .collect();

    // Verify steps exist specifically in the if.true branch executable lines.
    let if_branch_covered: Vec<i64> = if_branch_lines_1indexed
        .iter()
        .filter(|l| ncf_step_lines.contains(l))
        .copied()
        .collect();

    assert!(
        !if_branch_covered.is_empty(),
        "should have Step events at if.true branch executable lines {:?}, \
         but no matching steps found. All ncf steps: {:?}",
        if_branch_lines_1indexed, ncf_step_lines
    );

    // Verify steps exist specifically in the else branch executable lines.
    let else_branch_covered: Vec<i64> = else_branch_lines_1indexed
        .iter()
        .filter(|l| ncf_step_lines.contains(l))
        .copied()
        .collect();

    assert!(
        !else_branch_covered.is_empty(),
        "should have Step events at else branch executable lines {:?}, \
         but no matching steps found. All ncf steps: {:?}",
        else_branch_lines_1indexed, ncf_step_lines
    );

    // Verify both branches are specifically covered (not just one).
    assert!(
        !if_branch_covered.is_empty() && !else_branch_covered.is_empty(),
        "BOTH branches must have coverage. if.true covered: {:?}, else covered: {:?}",
        if_branch_covered, else_branch_covered
    );
}

// ---------------------------------------------------------------------------
// Test 12: Memory operations — verify mem_store/mem_load values and
// word-level memory operations
// ---------------------------------------------------------------------------

#[test]
fn test_miden_memory_operations() {
    let tmp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let out_dir = tmp_dir.path().join("traces");
    run_tracer(&out_dir);

    let events_arr = load_trace_events(&out_dir);

    // Collect all Int values emitted within the array_sum procedure context.
    let mut array_sum_values: Vec<i64> = Vec::new();
    walk_events_with_proc_context(&events_arr, |proc_name, event| {
        if !proc_name.ends_with("array_sum") {
            return;
        }
        if let Some(val) = event.get("Value") {
            if let Some(value) = val.get("value") {
                if value.get("kind").and_then(|k| k.as_str()) == Some("Int") {
                    if let Some(int_val) = value.get("i").and_then(|v| v.as_i64()) {
                        array_sum_values.push(int_val);
                    }
                }
            }
        }
    });

    // The array_sum procedure stores values 10, 20, 30, 40, 50 at addresses 100-104.
    // These values should appear as stack or local variable values during execution.
    let expected_array_values: [i64; 5] = [10, 20, 30, 40, 50];
    for &expected in &expected_array_values {
        assert!(
            array_sum_values.contains(&expected),
            "array_sum should capture value {} (from mem_store), got values: {:?}",
            expected,
            array_sum_values
        );
    }

    // The memory addresses 100-104 should appear as stack values (push.100, push.101, etc.).
    let expected_addresses: [i64; 5] = [100, 101, 102, 103, 104];
    for &addr in &expected_addresses {
        assert!(
            array_sum_values.contains(&addr),
            "array_sum should capture address {} (from push for mem_store), got values: {:?}",
            addr,
            array_sum_values
        );
    }

    // The final sum should be 150 (10+20+30+40+50).
    assert!(
        array_sum_values.contains(&150),
        "array_sum should produce final sum 150, got values: {:?}",
        array_sum_values
    );

    // Collect all Int values emitted within the memory_word_ops procedure context.
    let mut word_ops_values: Vec<i64> = Vec::new();
    walk_events_with_proc_context(&events_arr, |proc_name, event| {
        if !proc_name.ends_with("memory_word_ops") {
            return;
        }
        if let Some(val) = event.get("Value") {
            if let Some(value) = val.get("value") {
                if value.get("kind").and_then(|k| k.as_str()) == Some("Int") {
                    if let Some(int_val) = value.get("i").and_then(|v| v.as_i64()) {
                        word_ops_values.push(int_val);
                    }
                }
            }
        }
    });

    // memory_word_ops stores word [1, 2, 3, 4] at address 200.
    // These values should appear as stack values during execution.
    let expected_word: [i64; 4] = [1, 2, 3, 4];
    for &expected in &expected_word {
        assert!(
            word_ops_values.contains(&expected),
            "memory_word_ops should capture word element {} at address 200, got values: {:?}",
            expected,
            word_ops_values
        );
    }

    // The address 200 should appear as a stack value.
    assert!(
        word_ops_values.contains(&200),
        "memory_word_ops should capture address 200, got values: {:?}",
        word_ops_values
    );
}
