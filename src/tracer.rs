//! Tracer implementation for the Miden VM.
//!
//! Steps through a Miden program using `execute_iter` and emits
//! CodeTracer trace events (steps, calls, returns, variables).

use std::collections::HashMap;
use std::path::Path;

use codetracer_trace_types::{EventLogKind, Line, TypeKind, ValueRecord, NONE_VALUE};
use codetracer_trace_writer_nim::trace_writer::TraceWriter;
use codetracer_trace_writer_nim::{create_trace_writer, TraceEventsFileFormat};
use eyre::{eyre, Context, Result};
use miden_assembly::Assembler;
use miden_processor::{execute_iter, AsmOpInfo, DefaultHost, StackInputs, VmState};

use crate::source_map::SourceMap;

/// The main tracer struct that captures Miden VM execution traces.
pub struct MidenTracer {
    writer: Box<dyn TraceWriter + Send>,
    /// Miden "felt" type id (registered once).
    felt_type_id: Option<codetracer_trace_types::TypeId>,
}

impl MidenTracer {
    /// Trace a MASM program and write CodeTracer output files.
    ///
    /// 1. Assembles the MASM source in debug mode.
    /// 2. Executes with `execute_iter` to step through VM states.
    /// 3. Emits Step / Call / Return / Variable events.
    /// 4. Writes a `.ct` container file to `out_dir`.
    pub fn trace_program(
        source_path: &Path,
        source_code: &str,
        out_dir: &Path,
        format: TraceEventsFileFormat,
    ) -> Result<()> {
        let program_str = source_path.to_string_lossy();
        let writer = create_trace_writer(&program_str, &[], format);

        // Initialise output files.
        std::fs::create_dir_all(out_dir)
            .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

        let events_filename = match format {
            TraceEventsFileFormat::Json => "trace.json",
            TraceEventsFileFormat::Binary | TraceEventsFileFormat::BinaryV0 | TraceEventsFileFormat::Ctfs => "trace.bin",
        };
        let events_path = out_dir.join(events_filename);
        let metadata_path = out_dir.join("trace_metadata.json");
        let paths_path = out_dir.join("trace_paths.json");

        Self::trace_program_with_writer(source_path, source_code, writer, |w| {
            TraceWriter::begin_writing_trace_events(w, &events_path)
                .map_err(|e| eyre!("{e}"))?;
            TraceWriter::begin_writing_trace_metadata(w, &metadata_path)
                .map_err(|e| eyre!("{e}"))?;
            TraceWriter::begin_writing_trace_paths(w, &paths_path)
                .map_err(|e| eyre!("{e}"))?;
            Ok(())
        })?;
        Ok(())
    }

    /// Trace a MASM program using the provided writer.
    ///
    /// The `begin_fn` callback is invoked after writer creation to set up
    /// output streams (begin_writing_*). For file-backed writers this opens
    /// the output files; for in-memory test writers it can be a no-op.
    ///
    /// After tracing completes, the writer's finish and close methods are
    /// called automatically.
    pub fn trace_program_with_writer<F>(
        source_path: &Path,
        source_code: &str,
        writer: Box<dyn TraceWriter + Send>,
        begin_fn: F,
    ) -> Result<Box<dyn TraceWriter + Send>>
    where
        F: FnOnce(&mut (dyn TraceWriter + Send)) -> Result<()>,
    {
        // -- 1. Assemble in debug mode -----------------------------------------------
        let assembler = Assembler::default().with_debug_mode(true);
        let source_manager = assembler.source_manager();
        let program = assembler
            .assemble_program(source_path.to_path_buf())
            .map_err(|e| eyre!("assembly failed: {e}"))?;

        // -- 2. Execute with iterator -------------------------------------------------
        let stack_inputs = StackInputs::default();
        let mut host = DefaultHost::default();
        let vm_state_iter = execute_iter(&program, stack_inputs, &mut host, source_manager);

        // -- 3. Build source map for byte-offset -> line mapping ----------------------
        let source_map = SourceMap::from_source(source_path, source_code);

        // -- 4. Set up the tracer with the provided writer ----------------------------
        let mut tracer = MidenTracer {
            writer,
            felt_type_id: None,
        };

        // -- 5. Initialise output streams via callback --------------------------------
        begin_fn(&mut *tracer.writer)?;

        // -- 6. Start the trace (must be called before registering any types) -----------
        TraceWriter::start(&mut *tracer.writer, source_path, Line(1));

        // Register the "felt" type (after start, so that "None" gets TypeId(0)).
        let felt_type_id = TraceWriter::ensure_type_id(&mut *tracer.writer, TypeKind::Int, "felt");
        tracer.felt_type_id = Some(felt_type_id);

        // -- 7. Walk VM states --------------------------------------------------------
        tracer.process_vm_states(vm_state_iter, &source_map, source_path)?;

        // -- 8. Finish writing --------------------------------------------------------
        TraceWriter::finish_writing_trace_events(&mut *tracer.writer).map_err(|e| eyre!("{e}"))?;
        TraceWriter::finish_writing_trace_metadata(&mut *tracer.writer)
            .map_err(|e| eyre!("{e}"))?;
        TraceWriter::finish_writing_trace_paths(&mut *tracer.writer).map_err(|e| eyre!("{e}"))?;
        TraceWriter::close(&mut *tracer.writer).map_err(|e| eyre!("{e}"))?;

        Ok(tracer.writer)
    }

    /// Walk the VM state iterator, emitting trace events.
    fn process_vm_states(
        &mut self,
        vm_state_iter: miden_processor::VmStateIterator,
        source_map: &SourceMap,
        source_path: &Path,
    ) -> Result<()> {
        let felt_type_id = self.felt_type_id.unwrap();

        // Parse num_locals for each procedure from the MASM source.
        let proc_locals = parse_proc_locals(source_map.source_code());

        // Tracking state between iterations.
        let mut prev_line: Option<u32> = None;
        let mut prev_context_name: Option<String> = None;
        // Track which local memory slots have been written.
        let mut active_locals: HashMap<u32, ()> = HashMap::new();
        // Stack of context names for call/return tracking.
        let mut context_stack: Vec<String> = Vec::new();
        // Current procedure's num_locals for memory address calculation.
        let mut current_num_locals: u16 = 0;

        // Tracks whether the VM iterator surfaced an execution error; if so we
        // route it through `register_special_event(Error, ...)` so the
        // frontend's error channel surfaces the runtime failure (mirrors
        // Cairo 1.50 CairoPanic, Fuel 1.53 Panic/Revert and PolkaVM 1.55
        // Trap/Segfault routing). The trace is still finalised cleanly so
        // partial step / call records leading up to the failure remain
        // navigable in the calltrace pane.
        let mut vm_error: Option<String> = None;

        for result in vm_state_iter {
            let state: VmState = match result {
                Ok(s) => s,
                Err(e) => {
                    vm_error = Some(format!("{e}"));
                    break;
                }
            };

            let asmop: Option<&AsmOpInfo> = state.asmop.as_ref();

            // Skip cycles without assembly-level info.
            let asmop = match asmop {
                Some(a) => a,
                None => continue,
            };

            // Only process the first cycle of each assembly instruction to avoid duplicates.
            if asmop.cycle_idx() != 1 {
                continue;
            }

            let asm_ref: &miden_processor::AssemblyOp = asmop.as_ref();
            let op_str = asmop.op();
            let context_name = asmop.context_name().to_string();

            // -- Resolve source location -------------------------------------------------
            let line = if let Some(loc) = asm_ref.location() {
                let start_byte: u32 = loc.start.into();
                source_map.byte_to_line(start_byte)
            } else {
                // No location info — skip.
                continue;
            };

            // -- Call / Return detection via context_name changes ------------------------
            if prev_context_name.as_deref() != Some(&context_name) {
                if let Some(ref prev_ctx) = prev_context_name {
                    // Check if we are returning to a previous context.
                    if context_stack.last().map(|s| s.as_str()) == Some(&context_name) {
                        // Returning from prev_ctx back to the context on top of stack.
                        context_stack.pop();
                        let ret_val = NONE_VALUE;
                        TraceWriter::register_return(&mut *self.writer, ret_val);
                    } else {
                        // Entering a new context (call).
                        context_stack.push(prev_ctx.clone());

                        // Stage the visible operand-stack top as canonical
                        // call args (audit checklist (c)). Miden has no
                        // separate parameter list — procedures consume
                        // arguments off the operand stack — so the top-of-
                        // stack at the call boundary is the closest
                        // analogue to the calling-convention argument
                        // registers other recorders stage (PolkaVM 1.55
                        // A0..A5; Cairo 1.50 ContractCall calldata).
                        // We use names `s0`..`s3` to avoid colliding with
                        // the per-step `stack[i]` variable dump below.
                        // Pre-fix the recorder always passed `vec![]` here
                        // — the calltrace pane showed every procedure with
                        // empty arguments.
                        let arg_depth = state.stack.len().min(4);
                        for i in 0..arg_depth {
                            let int_val = state.stack[i].as_int() as i64;
                            let value = ValueRecord::Int {
                                i: int_val,
                                type_id: felt_type_id,
                            };
                            let _ = TraceWriter::arg(
                                &mut *self.writer,
                                &format!("s{i}"),
                                value,
                            );
                        }

                        let fn_id = TraceWriter::ensure_function_id(
                            &mut *self.writer,
                            &context_name,
                            source_path,
                            Line(line as i64),
                        );
                        TraceWriter::register_call(&mut *self.writer, fn_id, vec![]);
                    }
                }
                // Update num_locals for the new context.
                // The context_name from Miden may be prefixed (e.g. "#exec::compute"),
                // so we try the full name first, then the last segment.
                current_num_locals = proc_locals
                    .get(&context_name)
                    .or_else(|| {
                        // Extract the last segment after "::" or after "#exec::", etc.
                        context_name
                            .rsplit("::")
                            .next()
                            .and_then(|name| proc_locals.get(name))
                    })
                    .copied()
                    .unwrap_or(0);
                active_locals.clear();
                prev_context_name = Some(context_name.clone());
            }

            // -- Emit step if line changed -----------------------------------------------
            if prev_line != Some(line) {
                TraceWriter::register_step(&mut *self.writer, source_path, Line(line as i64));
                prev_line = Some(line);
            }

            // -- Track local memory slots ------------------------------------------------
            // When we see loc_store.N, mark slot N as active.
            if let Some(slot) = parse_loc_store(op_str) {
                active_locals.insert(slot, ());
            }

            // -- Emit stack top values as variables --------------------------------------
            let stack_depth = state.stack.len().min(4);
            for i in 0..stack_depth {
                let felt_val = state.stack[i];
                let int_val = felt_val.as_int() as i64;
                let name = format!("stack[{}]", i);
                let value = ValueRecord::Int {
                    i: int_val,
                    type_id: felt_type_id,
                };
                TraceWriter::register_variable_with_full_value(&mut *self.writer, &name, value);
            }

            // -- Emit local memory slot values -------------------------------------------
            // In Miden, local slot N is at address: fmp - (num_locals - N).
            // We look up each active slot's address in state.memory.
            if !active_locals.is_empty() && current_num_locals > 0 {
                let fmp = state.fmp.as_int();
                for &slot in active_locals.keys() {
                    let offset = current_num_locals as u64 - slot as u64;
                    if fmp >= offset {
                        let addr = (fmp - offset) as u32;
                        let target_addr = miden_processor::MemoryAddress::from(addr);
                        if let Some(&(_, felt_val)) =
                            state.memory.iter().find(|(a, _)| *a == target_addr)
                        {
                            let int_val = felt_val.as_int() as i64;
                            let name = format!("local[{}]", slot);
                            let value = ValueRecord::Int {
                                i: int_val,
                                type_id: felt_type_id,
                            };
                            TraceWriter::register_variable_with_full_value(
                                &mut *self.writer,
                                &name,
                                value,
                            );
                        }
                    }
                }
            }
        }

        // If the VM iterator surfaced an execution error, route it through
        // the structured error channel before draining the call stack so
        // the partial trace finalises with the failure recorded. Same
        // pattern as Cairo 1.50 CairoPanic, Cardano 1.48 AikenUplcEvalError,
        // Fuel 1.53 Panic/Revert and PolkaVM 1.55 Trap/Segfault.
        if let Some(ref message) = vm_error {
            eprintln!("Miden VM execution error: {message}");
            TraceWriter::register_special_event(
                &mut *self.writer,
                EventLogKind::Error,
                "miden_vm_error",
                message,
            );
        }

        // If we are still inside nested contexts, emit returns for them.
        while context_stack.pop().is_some() {
            TraceWriter::register_return(&mut *self.writer, NONE_VALUE);
        }

        // Emit the Return that closes the <toplevel> Call opened by start().
        // This must be unconditional: even if no VM states produced assembly
        // ops (empty program), the toplevel Call still needs to be closed.
        // Without it the db-backend crashes when step-over tries to find
        // the end of depth 0.
        TraceWriter::register_return(&mut *self.writer, NONE_VALUE);

        Ok(())
    }
}

/// Parse `loc_store.N` from an op string, returning the slot number N.
fn parse_loc_store(op: &str) -> Option<u32> {
    op.strip_prefix("loc_store.")
        .and_then(|s| s.parse::<u32>().ok())
}

/// Parse procedure declarations from MASM source to extract num_locals.
///
/// Returns a map from procedure name to num_locals.
/// Handles declarations like `proc.name.N` where N is the number of locals.
fn parse_proc_locals(source: &str) -> HashMap<String, u16> {
    let mut result = HashMap::new();
    for line in source.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("proc.") || trimmed.starts_with("export.") {
            // Format: proc.name.N or export.name.N
            let parts: Vec<&str> = trimmed.split('.').collect();
            if parts.len() >= 3 {
                let name = parts[1];
                if let Ok(num_locals) = parts[2].parse::<u16>() {
                    result.insert(name.to_string(), num_locals);
                }
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_loc_store() {
        assert_eq!(parse_loc_store("loc_store.0"), Some(0));
        assert_eq!(parse_loc_store("loc_store.3"), Some(3));
        assert_eq!(parse_loc_store("loc_store.42"), Some(42));
        assert_eq!(parse_loc_store("loc_load.0"), None);
        assert_eq!(parse_loc_store("push.10"), None);
        assert_eq!(parse_loc_store("add"), None);
    }
}
