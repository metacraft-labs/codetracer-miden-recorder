//! Integration tests for the Miden tracer.
//!
//! These tests use the in-memory NonStreamingTraceWriter to inspect trace
//! events directly, without going through file I/O.

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;

use codetracer_trace_types::*;
use codetracer_trace_writer_nim::non_streaming_trace_writer::NonStreamingTraceWriter;
use codetracer_trace_writer_nim::trace_writer::TraceWriter;

/// Helper: run the tracer on compute.masm and return the collected events.
fn run_tracer() -> Vec<TraceLowLevelEvent> {
    let masm_path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-programs/masm/compute.masm"
    ));
    let source_code = std::fs::read_to_string(masm_path).expect("failed to read compute.masm");

    let program_str = masm_path.to_string_lossy();
    let writer = NonStreamingTraceWriter::new(&program_str, &[]);
    let boxed_writer: Box<dyn TraceWriter + Send> = Box::new(writer);

    let returned_writer =
        codetracer_miden_recorder::tracer::MidenTracer::trace_program_with_writer(
            masm_path,
            &source_code,
            boxed_writer,
            |_w| Ok(()), // No file output needed for in-memory tests.
        )
        .expect("trace_program_with_writer should succeed");

    // Downcast the returned writer back to NonStreamingTraceWriter to read events.
    returned_writer.events().to_vec()
}

/// Read the compute.masm source and return it as a String.
fn read_compute_masm() -> String {
    let masm_path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/test-programs/masm/compute.masm"
    ));
    std::fs::read_to_string(masm_path).expect("failed to read compute.masm")
}

// ===========================================================================
// Shared helpers for event inspection
// ===========================================================================

/// Extract step lines from events.
fn step_lines(events: &[TraceLowLevelEvent]) -> Vec<i64> {
    events
        .iter()
        .filter_map(|e| match e {
            TraceLowLevelEvent::Step(s) => Some(s.line.0),
            _ => None,
        })
        .collect()
}

/// Extract function names from events.
fn function_names(events: &[TraceLowLevelEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            TraceLowLevelEvent::Function(f) => Some(f.name.clone()),
            _ => None,
        })
        .collect()
}

/// Build a map from FunctionId to function name.
fn build_function_id_map(events: &[TraceLowLevelEvent]) -> HashMap<usize, String> {
    let mut map = HashMap::new();
    let mut next_id: usize = 0;
    for event in events {
        if let TraceLowLevelEvent::Function(f) = event {
            map.insert(next_id, f.name.clone());
            next_id += 1;
        }
    }
    map
}

/// Walk events tracking the current procedure via Call/Return and invoke a
/// callback with (current_proc_name, event) for each event.
fn walk_events_with_proc_context<F>(events: &[TraceLowLevelEvent], mut callback: F)
where
    F: FnMut(&str, &TraceLowLevelEvent),
{
    let fn_map = build_function_id_map(events);
    let mut proc_stack: Vec<String> = vec!["main".to_string()];

    for event in events {
        match event {
            TraceLowLevelEvent::Call(c) => {
                if let Some(name) = fn_map.get(&c.function_id.0) {
                    proc_stack.push(name.clone());
                }
            }
            TraceLowLevelEvent::Return(_) => {
                if proc_stack.len() > 1 {
                    proc_stack.pop();
                }
            }
            _ => {}
        }

        let current = proc_stack.last().map(|s| s.as_str()).unwrap_or("unknown");
        callback(current, event);
    }
}

/// Extract Int values from Value events.
fn int_values(events: &[TraceLowLevelEvent]) -> Vec<i64> {
    events
        .iter()
        .filter_map(|e| match e {
            TraceLowLevelEvent::Value(fvr) => match &fvr.value {
                ValueRecord::Int { i, .. } => Some(*i),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Test 1: Basic execution - the tracer runs without error
// ---------------------------------------------------------------------------

#[test]
fn test_miden_tracer_basic_execution() {
    let events = run_tracer();

    // The trace should contain Step events (i.e., execution was actually recorded).
    let steps: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, TraceLowLevelEvent::Step(_)))
        .collect();
    assert!(
        !steps.is_empty(),
        "trace should contain at least one Step event, got none"
    );
}

// ---------------------------------------------------------------------------
// Test 2: Source mapping - step events map to correct MASM source lines
// ---------------------------------------------------------------------------

#[test]
fn test_miden_source_mapping() {
    let events = run_tracer();
    let lines = step_lines(&events);

    assert!(!lines.is_empty(), "there should be at least one Step event");

    // Derive the total line count from the source file at runtime.
    let source = read_compute_masm();
    let total_lines = source.lines().count() as i64;

    for &line in &lines {
        assert!(
            line >= 1 && line <= total_lines,
            "step line should be within compute.masm range (1-{}), got {}",
            total_lines,
            line
        );
    }

    // With the comprehensive program, we should have many more step events.
    assert!(
        lines.len() > 20,
        "comprehensive program should produce many step events, got {}",
        lines.len()
    );
}

// ---------------------------------------------------------------------------
// Test 3: Variable extraction - local memory slot values are captured
// ---------------------------------------------------------------------------

#[test]
fn test_miden_variable_extraction() {
    let events = run_tracer();

    // Should have Value events with Int values (from stack and local variables).
    let values = int_values(&events);
    assert!(
        !values.is_empty(),
        "there should be at least one Value event with an Int value"
    );

    // Should have many Value events from the comprehensive program.
    let value_count = events
        .iter()
        .filter(|e| matches!(e, TraceLowLevelEvent::Value(_)))
        .count();
    assert!(
        value_count > 50,
        "comprehensive program should produce many Value events, got {}",
        value_count
    );

    // Values should include known results from the program's procedures.
    assert!(
        values.contains(&55),
        "should contain fibonacci(10) = 55"
    );
    assert!(
        values.contains(&5040),
        "should contain factorial(7) = 5040"
    );
}

// ---------------------------------------------------------------------------
// Test 4: Output structure (verify events contain expected event types)
// ---------------------------------------------------------------------------

#[test]
fn test_miden_trace_event_types() {
    let events = run_tracer();

    let has_steps = events.iter().any(|e| matches!(e, TraceLowLevelEvent::Step(_)));
    let has_calls = events.iter().any(|e| matches!(e, TraceLowLevelEvent::Call(_)));
    let has_returns = events.iter().any(|e| matches!(e, TraceLowLevelEvent::Return(_)));
    let has_functions = events.iter().any(|e| matches!(e, TraceLowLevelEvent::Function(_)));
    let has_values = events.iter().any(|e| matches!(e, TraceLowLevelEvent::Value(_)));
    let has_paths = events.iter().any(|e| matches!(e, TraceLowLevelEvent::Path(_)));

    assert!(has_steps, "trace should have Step events");
    assert!(has_calls, "trace should have Call events");
    assert!(has_returns, "trace should have Return events");
    assert!(has_functions, "trace should have Function events");
    assert!(has_values, "trace should have Value events");
    assert!(has_paths, "trace should have Path events");
}

// ---------------------------------------------------------------------------
// Test 5: Multiple procedure calls appear in the trace
// ---------------------------------------------------------------------------

#[test]
fn test_miden_multiple_procedure_calls() {
    let events = run_tracer();

    let fn_names = function_names(&events);
    assert!(
        fn_names.len() >= 2,
        "should have at least 2 registered functions, got: {:?}",
        fn_names
    );

    let call_count = events
        .iter()
        .filter(|e| matches!(e, TraceLowLevelEvent::Call(_)))
        .count();
    let return_count = events
        .iter()
        .filter(|e| matches!(e, TraceLowLevelEvent::Return(_)))
        .count();

    assert!(call_count > 0, "should have at least one Call event");
    assert!(return_count > 0, "should have at least one Return event");
    assert_eq!(
        call_count, return_count,
        "Call and Return events should be balanced"
    );

    assert!(
        call_count >= 5,
        "comprehensive program should produce at least 5 Call events, got {}",
        call_count
    );
}

// ---------------------------------------------------------------------------
// Test 6: Local variable values from different procedures are captured
// ---------------------------------------------------------------------------

#[test]
fn test_miden_locals_from_multiple_procedures() {
    let events = run_tracer();
    let values = int_values(&events);

    assert!(
        values.len() > 50,
        "comprehensive program should produce many variable values, got {}",
        values.len()
    );

    // Verify some expected values appear (fibonacci sequence elements).
    assert!(
        values.contains(&55),
        "should see fibonacci(10) = 55 somewhere in values"
    );
}

// ---------------------------------------------------------------------------
// Test 7: Control flow through if/else branches is traced
// ---------------------------------------------------------------------------

#[test]
fn test_miden_control_flow_branches() {
    let events = run_tracer();
    let lines = step_lines(&events);

    let source = read_compute_masm();
    let source_lines: Vec<&str> = source.lines().collect();

    // Find the nested_control_flow procedure boundaries.
    let proc_start = source_lines
        .iter()
        .position(|l| l.trim().starts_with("proc.nested_control_flow"))
        .expect("should find proc.nested_control_flow");
    let proc_end = source_lines[proc_start..]
        .iter()
        .enumerate()
        .filter(|(i, l)| *i > 0 && l.trim() == "end")
        .last()
        .map(|(i, _)| proc_start + i)
        .expect("should find end of nested_control_flow");

    let if_true_offset = source_lines[proc_start..=proc_end]
        .iter()
        .position(|l| l.trim() == "if.true")
        .expect("should find if.true in nested_control_flow");
    let if_true_line = proc_start + if_true_offset;

    let else_offset = source_lines[proc_start..=proc_end]
        .iter()
        .position(|l| l.trim() == "else")
        .expect("should find else in nested_control_flow");
    let else_line = proc_start + else_offset;

    let if_end_offset = source_lines[(proc_start + else_offset)..]
        .iter()
        .position(|l| l.trim() == "end")
        .expect("should find end after else");
    let if_end_line = proc_start + else_offset + if_end_offset;

    let if_branch_start = (if_true_line + 2) as i64;
    let if_branch_end = else_line as i64;
    let else_branch_start = (else_line + 2) as i64;
    let else_branch_end = if_end_line as i64;

    let has_if_branch_steps = lines
        .iter()
        .any(|&l| l >= if_branch_start && l <= if_branch_end);
    let has_else_branch_steps = lines
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
    let events = run_tracer();
    let values = int_values(&events);

    assert!(values.contains(&55), "should contain fib(10) = 55");
    assert!(values.contains(&5040), "should contain factorial(7) = 5040");
    assert!(values.contains(&42), "should contain 42 from max_of_three");
    assert!(values.contains(&10), "should contain 10 from array_sum");
    assert!(values.contains(&125), "should contain 125 from arithmetic_demo");

    // Location-specific assertions: verify each value appears in events
    // associated with the correct procedure.
    let mut values_by_proc: HashMap<String, Vec<i64>> = HashMap::new();
    walk_events_with_proc_context(&events, |proc_name, event| {
        if let TraceLowLevelEvent::Value(fvr) = event {
            if let ValueRecord::Int { i, .. } = &fvr.value {
                values_by_proc
                    .entry(proc_name.to_string())
                    .or_default()
                    .push(*i);
            }
        }
    });

    let proc_has_value = |proc_suffix: &str, val: i64| -> bool {
        values_by_proc
            .iter()
            .any(|(name, vals)| name.ends_with(proc_suffix) && vals.contains(&val))
    };

    assert!(
        proc_has_value("fibonacci", 55),
        "value 55 should appear in fibonacci procedure events"
    );
    assert!(
        proc_has_value("factorial", 5040),
        "value 5040 should appear in factorial procedure events"
    );
    assert!(
        proc_has_value("max_of_three", 42),
        "value 42 should appear in max_of_three procedure events"
    );
    assert!(
        proc_has_value("array_sum", 10),
        "value 10 should appear in array_sum procedure events"
    );
    assert!(
        proc_has_value("arithmetic_demo", 125),
        "value 125 should appear in arithmetic_demo procedure events"
    );

    assert!(
        values_by_proc.len() >= 5,
        "should have values from at least 5 distinct procedures, got {}: {:?}",
        values_by_proc.len(),
        values_by_proc.keys().collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Test 9: Fibonacci value at location
// ---------------------------------------------------------------------------

#[test]
fn test_miden_fibonacci_value_at_location() {
    let events = run_tracer();
    let source = read_compute_masm();

    let source_lines: Vec<&str> = source.lines().collect();
    let fib_start = source_lines
        .iter()
        .position(|l| l.trim().starts_with("proc.fibonacci"))
        .expect("should find proc.fibonacci") + 1;
    let fib_body_end = source_lines[fib_start..]
        .iter()
        .position(|l| l.trim() == "end")
        .expect("should find end of fibonacci")
        + fib_start
        + 1;

    // Collect Step events within fibonacci lines.
    let fib_step_lines: Vec<i64> = events
        .iter()
        .filter_map(|e| match e {
            TraceLowLevelEvent::Step(s) => {
                let line = s.line.0;
                if line >= fib_start as i64 && line <= fib_body_end as i64 {
                    Some(line)
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    assert!(
        !fib_step_lines.is_empty(),
        "should have Step events within the fibonacci procedure (lines {}-{})",
        fib_start, fib_body_end
    );

    // Track values within fibonacci procedure context.
    let mut fib_values: Vec<i64> = Vec::new();
    let mut fib_step_count = 0usize;

    walk_events_with_proc_context(&events, |proc_name, event| {
        if !proc_name.ends_with("fibonacci") {
            return;
        }
        if matches!(event, TraceLowLevelEvent::Step(_)) {
            fib_step_count += 1;
        }
        if let TraceLowLevelEvent::Value(fvr) = event {
            if let ValueRecord::Int { i, .. } = &fvr.value {
                fib_values.push(*i);
            }
        }
    });

    assert!(fib_step_count > 0, "fibonacci should produce Step events");
    assert!(
        fib_values.contains(&55),
        "fibonacci procedure should produce value 55 (fib(10)), got values: {:?}",
        fib_values
    );

    let fib_sequence = [1i64, 2, 3, 5, 8, 13, 21, 34, 55];
    let found_fib_values: Vec<i64> = fib_sequence
        .iter()
        .filter(|v| fib_values.contains(v))
        .copied()
        .collect();
    assert!(
        found_fib_values.len() >= 3,
        "should see at least 3 fibonacci sequence values, found: {:?}",
        found_fib_values
    );
}

// ---------------------------------------------------------------------------
// Test 10: Call tree structure
// ---------------------------------------------------------------------------

#[test]
fn test_miden_call_tree_structure() {
    let events = run_tracer();
    let fn_map = build_function_id_map(&events);

    // Verify all expected procedures are registered.
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

    // Reconstruct call tree.
    let mut call_tree: Vec<(String, String)> = Vec::new();
    for event in &events {
        match event {
            TraceLowLevelEvent::Call(c) => {
                if let Some(name) = fn_map.get(&c.function_id.0) {
                    call_tree.push(("Call".to_string(), name.clone()));
                }
            }
            TraceLowLevelEvent::Return(_) => {
                call_tree.push(("Return".to_string(), String::new()));
            }
            _ => {}
        }
    }

    let call_count = call_tree.iter().filter(|(t, _)| t == "Call").count();
    let return_count = call_tree.iter().filter(|(t, _)| t == "Return").count();
    assert_eq!(
        call_count, return_count,
        "Call and Return events must be balanced: {} calls vs {} returns",
        call_count, return_count
    );

    // Verify call order matches program structure.
    let call_order: Vec<String> = call_tree
        .iter()
        .filter(|(t, _)| t == "Call")
        .map(|(_, name)| name.rsplit("::").next().unwrap_or(name).to_string())
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

    let mut order_idx = 0;
    for call_name in &call_order {
        if order_idx < expected_order.len() && call_name == expected_order[order_idx] {
            order_idx += 1;
        }
    }
    assert_eq!(
        order_idx,
        expected_order.len(),
        "call order should match expected program structure. Expected {:?}, got: {:?}",
        expected_order,
        call_order
    );

    // Verify proper nesting.
    let mut depth: i32 = 0;
    for (event_type, _) in &call_tree {
        match event_type.as_str() {
            "Call" => depth += 1,
            "Return" => {
                depth -= 1;
                assert!(depth >= 0, "Return without matching Call");
            }
            _ => {}
        }
    }
    assert_eq!(depth, 0, "call stack should be empty at end of trace");
}

// ---------------------------------------------------------------------------
// Test 11: Conditional branch coverage
// ---------------------------------------------------------------------------

#[test]
fn test_miden_conditional_branch_coverage() {
    let events = run_tracer();
    let source = read_compute_masm();
    let source_lines: Vec<&str> = source.lines().collect();

    let proc_start = source_lines
        .iter()
        .position(|l| l.trim().starts_with("proc.nested_control_flow"))
        .expect("should find proc.nested_control_flow");

    let if_true_idx = source_lines[proc_start..]
        .iter()
        .position(|l| l.trim() == "if.true")
        .expect("should find if.true")
        + proc_start;

    let else_idx = source_lines[if_true_idx..]
        .iter()
        .position(|l| l.trim() == "else")
        .expect("should find else")
        + if_true_idx;

    let if_end_idx = source_lines[else_idx..]
        .iter()
        .position(|l| l.trim() == "end")
        .expect("should find end after else")
        + else_idx;

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

    assert!(!if_branch_executable_lines.is_empty());
    assert!(!else_branch_executable_lines.is_empty());

    // Collect step lines within nested_control_flow context.
    let mut ncf_step_lines: Vec<i64> = Vec::new();
    walk_events_with_proc_context(&events, |proc_name, event| {
        if !proc_name.ends_with("nested_control_flow") {
            return;
        }
        if let TraceLowLevelEvent::Step(s) = event {
            ncf_step_lines.push(s.line.0);
        }
    });

    let if_branch_lines_1indexed: Vec<i64> = if_branch_executable_lines
        .iter()
        .map(|&i| (i + 1) as i64)
        .collect();
    let else_branch_lines_1indexed: Vec<i64> = else_branch_executable_lines
        .iter()
        .map(|&i| (i + 1) as i64)
        .collect();

    let if_branch_covered: Vec<i64> = if_branch_lines_1indexed
        .iter()
        .filter(|l| ncf_step_lines.contains(l))
        .copied()
        .collect();
    let else_branch_covered: Vec<i64> = else_branch_lines_1indexed
        .iter()
        .filter(|l| ncf_step_lines.contains(l))
        .copied()
        .collect();

    assert!(
        !if_branch_covered.is_empty(),
        "should have Step events at if.true branch lines {:?}, ncf steps: {:?}",
        if_branch_lines_1indexed, ncf_step_lines
    );
    assert!(
        !else_branch_covered.is_empty(),
        "should have Step events at else branch lines {:?}, ncf steps: {:?}",
        else_branch_lines_1indexed, ncf_step_lines
    );
}

// ---------------------------------------------------------------------------
// Test 12: Memory operations
// ---------------------------------------------------------------------------

#[test]
fn test_miden_memory_operations() {
    let events = run_tracer();

    let mut array_sum_values: Vec<i64> = Vec::new();
    walk_events_with_proc_context(&events, |proc_name, event| {
        if !proc_name.ends_with("array_sum") {
            return;
        }
        if let TraceLowLevelEvent::Value(fvr) = event {
            if let ValueRecord::Int { i, .. } = &fvr.value {
                array_sum_values.push(*i);
            }
        }
    });

    let expected_array_values: [i64; 5] = [10, 20, 30, 40, 50];
    for &expected in &expected_array_values {
        assert!(
            array_sum_values.contains(&expected),
            "array_sum should capture value {}, got values: {:?}",
            expected,
            array_sum_values
        );
    }

    let expected_addresses: [i64; 5] = [100, 101, 102, 103, 104];
    for &addr in &expected_addresses {
        assert!(
            array_sum_values.contains(&addr),
            "array_sum should capture address {}, got values: {:?}",
            addr,
            array_sum_values
        );
    }

    assert!(
        array_sum_values.contains(&150),
        "array_sum should produce final sum 150, got values: {:?}",
        array_sum_values
    );

    let mut word_ops_values: Vec<i64> = Vec::new();
    walk_events_with_proc_context(&events, |proc_name, event| {
        if !proc_name.ends_with("memory_word_ops") {
            return;
        }
        if let TraceLowLevelEvent::Value(fvr) = event {
            if let ValueRecord::Int { i, .. } = &fvr.value {
                word_ops_values.push(*i);
            }
        }
    });

    let expected_word: [i64; 4] = [1, 2, 3, 4];
    for &expected in &expected_word {
        assert!(
            word_ops_values.contains(&expected),
            "memory_word_ops should capture word element {}, got values: {:?}",
            expected,
            word_ops_values
        );
    }

    assert!(
        word_ops_values.contains(&200),
        "memory_word_ops should capture address 200, got values: {:?}",
        word_ops_values
    );
}
