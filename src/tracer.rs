//! Tracer implementation for the Miden VM.
//!
//! Steps through a Miden program using `execute_iter` and emits
//! CodeTracer trace events (steps, calls, returns, variables).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use codetracer_trace_types::{EventLogKind, FunctionId, Line, NONE_VALUE, TypeKind, ValueRecord};
use codetracer_trace_writer_nim::trace_writer::TraceWriter;
use codetracer_trace_writer_nim::{TraceEventsFileFormat, create_trace_writer};
use eyre::{Context, Result, eyre};
use miden_assembly::Assembler;
use miden_processor::{AsmOpInfo, DefaultHost, StackInputs, VmState, execute_iter};

use crate::source_map::SourceMap;

/// The on-disk container produced by the recorder is always the canonical
/// multi-stream CTFS bundle.  Pre-2026-05-08 the recorder accepted a
/// `TraceEventsFileFormat` parameter and the CLI exposed a `--format` flag;
/// the convention now mandates CTFS-only output (see
/// `Recorder-CLI-Conventions.md` §4 in `codetracer-specs`).
const TRACE_FORMAT: TraceEventsFileFormat = TraceEventsFileFormat::Ctfs;

/// The main tracer struct that captures Miden VM execution traces.
pub struct MidenTracer {
    writer: Box<dyn TraceWriter + Send>,
    /// Miden "felt" type id (registered once).
    felt_type_id: Option<codetracer_trace_types::TypeId>,
    /// Miden "Word" type id -- a 4-felt compound value emitted as
    /// `ValueRecord::Sequence` after every `mem_loadw` / `loc_loadw`.
    word_type_id: Option<codetracer_trace_types::TypeId>,
}

impl MidenTracer {
    /// Trace a MASM program and write a CodeTracer CTFS trace bundle.
    ///
    /// 1. Assembles the MASM source in debug mode.
    /// 2. Executes with `execute_iter` to step through VM states.
    /// 3. Emits Step / Call / Return / Variable events.
    /// 4. Writes a multi-stream `.ct` container file to `out_dir`.
    ///
    /// The output format is fixed to CTFS — see
    /// `Recorder-CLI-Conventions.md` §4 in `codetracer-specs`.  Use
    /// `ct print` (from `codetracer-trace-format-nim`) for human-readable
    /// conversion of the produced bundle.
    pub fn trace_program(source_path: &Path, source_code: &str, out_dir: &Path) -> Result<()> {
        let program_str = source_path.to_string_lossy();
        let writer = create_trace_writer(&program_str, &[], TRACE_FORMAT);

        // Initialise output files.
        std::fs::create_dir_all(out_dir)
            .with_context(|| format!("cannot create output dir: {}", out_dir.display()))?;

        // CTFS multi-stream container — `db-backend` infers the format
        // from the `.bin` extension.  No JSON / legacy-binary alternative
        // is exposed.
        let events_path = out_dir.join("trace.bin");
        let metadata_path = out_dir.join("trace_metadata.json");
        let paths_path = out_dir.join("trace_paths.json");

        Self::trace_program_with_writer(source_path, source_code, writer, |w| {
            TraceWriter::begin_writing_trace_events(w, &events_path).map_err(|e| eyre!("{e}"))?;
            TraceWriter::begin_writing_trace_metadata(w, &metadata_path)
                .map_err(|e| eyre!("{e}"))?;
            TraceWriter::begin_writing_trace_paths(w, &paths_path).map_err(|e| eyre!("{e}"))?;
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
            word_type_id: None,
        };

        // -- 5. Initialise output streams via callback --------------------------------
        begin_fn(&mut *tracer.writer)?;

        // -- 6. Start the trace (must be called before registering any types) -----------
        TraceWriter::start(&mut *tracer.writer, source_path, Line(1));

        // Register the "felt" type (after start, so that "None" gets TypeId(0)).
        let felt_type_id = TraceWriter::ensure_type_id(&mut *tracer.writer, TypeKind::Int, "felt");
        tracer.felt_type_id = Some(felt_type_id);

        // Register the "Word" type -- the canonical 4-felt Miden
        // compound value surfaced after every `mem_loadw` /
        // `loc_loadw`.  Registering eagerly (alongside `felt`)
        // keeps the type table compact and the type id stable
        // across all recorded programs.
        let word_type_id = TraceWriter::ensure_type_id(&mut *tracer.writer, TypeKind::Seq, "Word");
        tracer.word_type_id = Some(word_type_id);

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
        let word_type_id = self.word_type_id.unwrap();

        // Parse num_locals for each procedure from the MASM source.
        let proc_locals = parse_proc_locals(source_map.source_code());
        // Parse the static call graph so we can disambiguate "deeper
        // call" from "sibling-after-return" at runtime — see
        // `parse_call_graph`'s docs.
        let call_graph = parse_call_graph(source_map.source_code());
        let parent_map = build_parent_map(&call_graph);

        // Pre-register every declared procedure in the function table.
        // The runtime context-name observation only surfaces procedures
        // whose body is actually executed at an asmop boundary; the
        // assembler may inline thin wrappers (e.g. `proc.compute.0 {
        // exec.outer }`) so their bodies share the inlined callee's
        // context name.  Without this static-decl pass, those inlined
        // procedures are silently dropped from the functions table —
        // see `test_nested_calls_full_chain_registered`.  We use the
        // same `#exec::` prefix the runtime would assign so the
        // function lookups stay consistent.
        let mut function_ids: HashMap<String, FunctionId> = HashMap::new();
        for proc_name in proc_locals.keys() {
            let prefixed = format!("#exec::{}", proc_name);
            let fid =
                TraceWriter::ensure_function_id(&mut *self.writer, &prefixed, source_path, Line(1));
            function_ids.insert(prefixed, fid);
        }

        // Tracking state between iterations.
        let mut prev_line: Option<u32> = None;
        let mut prev_context_name: Option<String> = None;
        // The op_str that started the most recent step we emitted.  When
        // execution stays on the same line but revisits this op_str
        // AFTER having moved past it (saw a different op since the
        // step), it means a `repeat.N` body (or any other backwards
        // branch into the same source line) has wrapped to a new
        // iteration — without this signal the line-change dedup below
        // would collapse every iteration into a single step event,
        // which the GUI's step-over cannot navigate through.  Requiring
        // `step_moved_past_first` guards against legitimate single-line
        // bodies where the same op appears twice in a row (e.g. `add
        // add` on one line) without an actual loop-back. See
        // `test_control_flow_repeat_emits_step_per_iteration`.
        let mut step_first_op: Option<String> = None;
        let mut step_moved_past_first: bool = false;
        // Track which local memory slots have been written.
        let mut active_locals: HashMap<u32, ()> = HashMap::new();
        // Stack of context names for call/return tracking.
        let mut context_stack: Vec<String> = Vec::new();
        // Current procedure's num_locals for memory address calculation.
        let mut current_num_locals: u16 = 0;
        // (`function_ids` cache is initialised above with the static
        // pre-registration of every declared procedure — see the
        // `proc_locals` loop above.)  This cache deduplicates by
        // `context_name`: the Nim FFI's `ensure_function_id` keys on
        // (name, path, line), so calling it from a *different* asmop
        // line for the same procedure would otherwise mint a fresh,
        // non-interned function id (a writer-level quirk that bites
        // the inner call-emitting branch below if the same procedure
        // is re-entered from a different asmop boundary).
        // Set when the recorder synthesised an explicit
        // `register_call(#main)` for the begin-block — we need to emit
        // a matching `register_return` after the main loop so the
        // call/return event count stays balanced (and the writer's
        // call stack closes #main before the final
        // close-the-toplevel-frame return below).
        let mut synthesised_main_call = false;
        // Set when the recorder synthesised an inlined-chain
        // `register_call` for the leaf procedure observed first (e.g.
        // `inner` in `nested_calls_test`).  The intermediate callers
        // are pushed onto `context_stack`; the leaf is the writer's
        // current top.  We need an extra `register_return` at end-of-
        // trace to close the leaf, mirroring the synthesised call.
        let mut synthesised_chain_leaf = false;

        // Tracks whether the VM iterator surfaced an execution error; if so we
        // route it through `register_special_event(Error, ...)` so the
        // frontend's error channel surfaces the runtime failure (mirrors
        // Cairo 1.50 CairoPanic, Fuel 1.53 Panic/Revert and PolkaVM 1.55
        // Trap/Segfault routing). The trace is still finalised cleanly so
        // partial step / call records leading up to the failure remain
        // navigable in the calltrace pane.
        let mut vm_error: Option<String> = None;

        // Pending Word value harvested at the LAST cycle of a
        // multi-cycle Word-load asmop (`loc_loadw`, which is 3
        // cycles, sometimes 4).  The recorder normally only
        // processes cycle_idx == 1 of each asmop, but the
        // post-load state for `loc_loadw` only materialises at
        // cycle_idx == num_cycles, so we capture the word here
        // and emit it as a `register_variable_with_full_value`
        // on the next cycle_idx == 1 boundary (which is the next
        // asmop's BEFORE-state -- i.e. the post-loadw state).
        let mut pending_word: Option<Vec<i64>> = None;

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

            // Capture the post-load Word from the LAST cycle of a
            // multi-cycle word-load (loc_loadw); for single-cycle
            // mem_loadw this also fires (cycle_idx == 1 == num_cycles)
            // so the pending_word path covers both kinds of word
            // load.  We harvest unconditionally here -- the emission
            // happens on the next cycle_idx == 1 boundary below.
            if asmop.cycle_idx() == asmop.num_cycles()
                && is_word_load(asmop.op())
                && state.stack.len() >= 4
            {
                pending_word = Some((0..4).map(|i| state.stack[i].as_int() as i64).collect());
            }

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
                // Register the newly-observed procedure in the function
                // table unconditionally — every procedure that surfaces
                // as a `context_name` (including the very first one,
                // which has `prev_context_name == None`) must appear in
                // the trace's `functions` table.  Cross-recorder
                // convention: PHP / Move / Cardano / Cairo all register
                // every entered procedure regardless of whether the
                // recorder also emits a `call_entry` for it.  Without
                // this hook the first procedure observed (the entry
                // point of the program, e.g. `mem_writer` when `begin`
                // does `exec.mem_writer ...`) was silently dropped from
                // the function table because the call/return-detection
                // branch below only fires on a transition from a
                // *known* previous context.
                //
                // The cache deduplicates by `context_name`: the Nim
                // FFI's `ensure_function_id` keys on (name, path, line),
                // so calling it from a *different* asmop line for the
                // same procedure would otherwise mint a fresh,
                // non-interned function id (a writer-level quirk that
                // also bites the inner call-emitting branch below if
                // the same procedure is re-entered from a different
                // asmop boundary).
                let new_fn_id = if let Some(&existing) = function_ids.get(&context_name) {
                    existing
                } else {
                    let fid = TraceWriter::ensure_function_id(
                        &mut *self.writer,
                        &context_name,
                        source_path,
                        Line(line as i64),
                    );
                    function_ids.insert(context_name.clone(), fid);
                    fid
                };

                if prev_context_name.is_none() {
                    let bare = bare_proc_name(&context_name);
                    if bare == "#main" {
                        // Synthesise an explicit `register_call` for
                        // the implicit `#main` (the begin-block) when
                        // it is the first observed context.  Without
                        // this, the LIFO exit ordering test cannot
                        // satisfy the "outermost-closed-last"
                        // invariant — `#main` would never appear on
                        // the writer's call stack, so the
                        // end-of-trace drain would close child
                        // procedures (drained from `context_stack`)
                        // instead of #main.  The complement is the
                        // explicit `register_return` for `#main` at
                        // end-of-trace below; together they encode
                        // `#main` as a real outermost call frame.
                        // See `test_control_flow_call_exit_strict_lifo`.
                        TraceWriter::register_call(&mut *self.writer, new_fn_id, vec![]);
                        synthesised_main_call = true;
                    } else if let Some(chain) = chain_from_main(&parent_map, bare) {
                        // The first observed context is buried under a
                        // chain of inlined wrappers (`compute → outer →
                        // middle → inner` where the assembler skipped
                        // every transition above `inner`).  Synthesise
                        // a `register_call` for every ancestor in the
                        // chain (excluding the implicit `#main`
                        // toplevel) so the inlined wrappers surface as
                        // call frames at runtime.  `context_stack`
                        // gets the chain's intermediate callers (so
                        // the natural `register_return` flow when
                        // execution unwinds back through `middle →
                        // outer → ...` matches the synthesised
                        // depths).  See
                        // `test_nested_calls_full_chain_registered`.
                        if chain.len() > 2 {
                            // Skip first (`#main`) and last (already
                            // emitted by the natural-call path below
                            // via `new_fn_id` register_call …
                            // actually no — we need to register the
                            // entire chain here including the leaf,
                            // because the `if let Some(ref prev_ctx)`
                            // branch below is gated on a non-None
                            // prev_context_name).  So we emit a call
                            // for `#main`'s child through to the leaf
                            // (inclusive); the recorder's state only
                            // tracks depth via context_stack so the
                            // intermediate callers (everything except
                            // the leaf) get pushed.
                            for ancestor in &chain[1..chain.len() - 1] {
                                let prefixed = format!("#exec::{}", ancestor);
                                let fid =
                                    function_ids.get(&prefixed).copied().unwrap_or_else(|| {
                                        let f = TraceWriter::ensure_function_id(
                                            &mut *self.writer,
                                            &prefixed,
                                            source_path,
                                            Line(line as i64),
                                        );
                                        function_ids.insert(prefixed.clone(), f);
                                        f
                                    });
                                TraceWriter::register_call(&mut *self.writer, fid, vec![]);
                                context_stack.push(prefixed);
                            }
                            // Finally register the call for the leaf
                            // (the actually-observed first context).
                            TraceWriter::register_call(&mut *self.writer, new_fn_id, vec![]);
                            synthesised_chain_leaf = true;
                        }
                    }
                }
                if let Some(ref prev_ctx) = prev_context_name {
                    // Check if we are returning to a previous context.
                    if context_stack.last().map(|s| s.as_str()) == Some(&context_name) {
                        // Returning from prev_ctx back to the context on top of stack.
                        context_stack.pop();
                        let ret_val = NONE_VALUE;
                        TraceWriter::register_return(&mut *self.writer, ret_val);
                    } else if synthesised_chain_leaf
                        && bare_proc_name(&context_name) == "#main"
                        && !context_stack.iter().any(|c| bare_proc_name(c) == "#main")
                    {
                        // Inlined-chain end-of-trace: the begin-block
                        // (`#main`) only surfaces for the cleanup
                        // `drop drop drop` after the deepest call
                        // chain unwinds.  Drain every still-open
                        // synthesised frame (each pop on
                        // `context_stack` matches a `register_return`
                        // that closes the writer's current top), plus
                        // one extra return for the outermost
                        // synthesised caller (the entry on
                        // `context_stack` we just popped maps to the
                        // writer frame BELOW our current top — see
                        // the trace in
                        // `test_nested_calls_full_chain_registered`).
                        // Do NOT register a `call_entry` for `#main`
                        // here: it is the implicit toplevel and the
                        // expected 4-call chain already accounts for
                        // every distinct frame.
                        while context_stack.pop().is_some() {
                            TraceWriter::register_return(&mut *self.writer, NONE_VALUE);
                        }
                        // Close the leaf (the outermost synthesised
                        // caller is now the writer's top).  We rely on
                        // the unconditional `register_return` at end-
                        // of-trace below to close the toplevel.
                        TraceWriter::register_return(&mut *self.writer, NONE_VALUE);
                        // Mark the synthesised leaf as already closed
                        // so the end-of-trace cleanup doesn't double-
                        // emit a return for it.
                        synthesised_chain_leaf = false;
                    } else {
                        // Disambiguate: is this a deeper call into `ctx`,
                        // or a sibling transition where `prev` returned
                        // and the same parent now invoked `ctx`?  We can
                        // tell only when the call stack's top (the
                        // current caller) is known via static MASM
                        // parsing to invoke BOTH `prev` and `ctx` — in
                        // that case the assembler skipped the implicit
                        // return-to-parent boundary, so emit the missing
                        // `register_return` for `prev` before opening
                        // `ctx`.  This keeps `call_exit` ordering strict
                        // LIFO (innermost first) for sequences like
                        // `begin exec.A exec.B end` rather than draining
                        // every sibling in reverse-callKey order at
                        // end-of-trace.  See
                        // `test_control_flow_call_exit_strict_lifo`.
                        //
                        // The fallback branch (push `prev`, open `ctx`)
                        // handles genuinely deeper calls whose
                        // intermediate frames the assembler inlined —
                        // see `test_nested_calls_test_via_ct_print_full`
                        // where `compute → outer → middle → inner`
                        // surfaces only as the deepest context.
                        let caller_invokes_both = context_stack
                            .last()
                            .and_then(|caller| call_graph.get(bare_proc_name(caller)))
                            .map(|callees| {
                                callees.contains(bare_proc_name(prev_ctx))
                                    && callees.contains(bare_proc_name(&context_name))
                            })
                            .unwrap_or(false);

                        if caller_invokes_both {
                            // Sibling: close the previous call first.
                            TraceWriter::register_return(&mut *self.writer, NONE_VALUE);
                            // Stage call args from the operand stack — same
                            // logic as the deeper-call branch below.
                            let arg_depth = state.stack.len().min(4);
                            for i in 0..arg_depth {
                                let int_val = state.stack[i].as_int() as i64;
                                let value = ValueRecord::Int {
                                    i: int_val,
                                    type_id: felt_type_id,
                                };
                                let _ =
                                    TraceWriter::arg(&mut *self.writer, &format!("s{i}"), value);
                            }
                            TraceWriter::register_call(&mut *self.writer, new_fn_id, vec![]);
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
                                let _ =
                                    TraceWriter::arg(&mut *self.writer, &format!("s{i}"), value);
                            }

                            TraceWriter::register_call(&mut *self.writer, new_fn_id, vec![]);
                        }
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

            // -- Emit step on line change OR loop-iteration restart ---------------------
            // A `repeat.N` body whose instructions all live on the same
            // source line revisits the FIRST op of that line on every
            // iteration.  Detecting `op_str == step_first_op` while
            // `line == prev_line` AND `step_moved_past_first` is the
            // recorder's only signal that a backwards branch fired
            // without crossing a line boundary — emit a step so the
            // per-iteration step count matches the dynamic loop trip
            // count.  The `moved_past_first` guard avoids false-firing
            // on legitimate single-line bodies where the same op
            // appears twice in a row (e.g. `add add` on one line).
            // See `test_control_flow_repeat_emits_step_per_iteration`.
            let line_changed = prev_line != Some(line);
            let iteration_wrap =
                !line_changed && step_moved_past_first && step_first_op.as_deref() == Some(op_str);
            if line_changed || iteration_wrap {
                TraceWriter::register_step(&mut *self.writer, source_path, Line(line as i64));
                prev_line = Some(line);
                step_first_op = Some(op_str.to_string());
                step_moved_past_first = false;
            } else if step_first_op.as_deref() != Some(op_str) {
                step_moved_past_first = true;
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

            // -- Emit Word value at a mem_loadw / loc_loadw -------------------------------
            // The recorder normally observes VM state at
            // cycle_idx == 1 of each asmop.  For a single-cycle
            // word load (`mem_loadw`, 1 cycle) cycle_idx == 1 IS
            // num_cycles, so the post-load state is visible
            // directly via `state.stack`.  For a multi-cycle
            // word load (`loc_loadw`, 3 cycles) we captured the
            // post-load word above when iterating through the
            // ordinarily-skipped cycles; emit it here at the
            // current asmop boundary (which is the SAME asmop
            // that issued the load -- pending_word and op_str
            // align on the same step boundary).
            //
            // Either path yields a `ValueRecord::Sequence` typed
            // against the eagerly-registered `Word` type so the
            // canonical Miden compound value is preserved (precondition
            // for hash digest typing and Word-aware GUI rendering).
            // See `memory_word_ops_test.masm` and
            // `local_word_ops_test.masm`.
            // Drain the pending word first (set by a multi-cycle
            // word load whose post-load state landed on a
            // cycle_idx > 1 above).  If no pending word is set
            // and the CURRENT asmop is a single-cycle word load
            // (`mem_loadw` is 1 cycle, so cycle_idx==1 IS the
            // post-load state), use the live `state.stack`.  We
            // do NOT fall back to the live stack for multi-cycle
            // word loads because at cycle_idx==1 the load has not
            // yet completed; the pending_word path will surface
            // the correct value on the next asmop's boundary.
            let word_to_emit: Option<Vec<i64>> = if let Some(w) = pending_word.take() {
                Some(w)
            } else if is_word_load(op_str) && asmop.num_cycles() == 1 && state.stack.len() >= 4 {
                Some((0..4).map(|i| state.stack[i].as_int() as i64).collect())
            } else {
                None
            };
            if let Some(elements) = word_to_emit {
                let elements: Vec<ValueRecord> = elements
                    .into_iter()
                    .map(|i| ValueRecord::Int {
                        i,
                        type_id: felt_type_id,
                    })
                    .collect();
                let word_value = ValueRecord::Sequence {
                    elements,
                    is_slice: false,
                    type_id: word_type_id,
                };
                TraceWriter::register_variable_with_full_value(
                    &mut *self.writer,
                    "word",
                    word_value,
                );
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

        // If the recorder synthesised an explicit `register_call(#main)`
        // at first-observation, close it now — its complement (see the
        // call-detection block above).  This must precede the
        // close-the-toplevel-frame return below so the writer's call
        // stack drains in the right order.
        if synthesised_main_call {
            TraceWriter::register_return(&mut *self.writer, NONE_VALUE);
        }
        // Same for the inlined-chain leaf: close it so the writer's
        // call/return event count stays balanced.
        if synthesised_chain_leaf {
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

/// Returns true if the op string is a Word-level memory load
/// (`mem_loadw`, `mem_loadw.<addr>`, `loc_loadw.<slot>`) -- the
/// instruction that surfaces a 4-felt Word as the top of the
/// operand stack.  After such an op, the top four stack felts are
/// the canonical Miden Word value and the recorder surfaces them
/// as a `ValueRecord::Sequence` (the Miden compound value
/// analogous to a Cairo felt-word, a Move struct, or an EVM
/// 256-bit word).
fn is_word_load(op: &str) -> bool {
    op == "mem_loadw"
        || op.starts_with("mem_loadw.")
        || op == "loc_loadw"
        || op.starts_with("loc_loadw.")
}

/// Distinguishes the three Miden procedure-invocation forms:
///
/// * `Exec`    — `exec.X`     (inline expansion, same memory context)
/// * `Call`    — `call.X`     (cross-context call, fresh memory context)
/// * `SysCall` — `syscall.X`  (kernel-procedure call)
///
/// The recorder uses this kind tag to label call_entry events so
/// the calltrace pane can distinguish a logical inline call from
/// a true cross-context boundary -- matching the Cairo
/// `ContractCall`/`LibraryCall` distinction and the EVM
/// `CALL`/`STATICCALL`/`DELEGATECALL` distinction surfaced by
/// peer recorders.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum CallKind {
    Exec,
    Call,
    SysCall,
}

impl CallKind {
    pub fn as_tag(&self) -> &'static str {
        match self {
            CallKind::Exec => "Exec",
            CallKind::Call => "Call",
            CallKind::SysCall => "SysCall",
        }
    }
}

/// Static parse of (caller -> callee -> CallKind) from MASM source.
/// Public for direct testing of the kind-detection logic
/// independently of the recorder's runtime path -- the runtime
/// trace still emits a single call_entry per call boundary; the
/// kind tag is exposed via `parse_call_kinds` so external tools
/// (and the per-fixture strict tests) can join the static kind
/// metadata onto the runtime call sequence.
pub fn parse_call_kinds(source: &str) -> HashMap<String, HashMap<String, CallKind>> {
    let mut graph: HashMap<String, HashMap<String, CallKind>> = HashMap::new();
    let mut current: Option<String> = None;
    for line in source.lines() {
        let trimmed = line.trim();
        let code = match trimmed.find('#') {
            Some(idx) => &trimmed[..idx],
            None => trimmed,
        };
        let code = code.trim();
        if code.is_empty() {
            continue;
        }
        if code.starts_with("proc.") || code.starts_with("export.") {
            if let Some(name) = code.split('.').nth(1) {
                current = Some(name.to_string());
                graph.entry(name.to_string()).or_default();
            }
        } else if code == "begin" {
            current = Some("#main".to_string());
            graph.entry("#main".to_string()).or_default();
        } else if code == "end" {
            current = None;
        } else if let Some(parent) = current.as_ref() {
            for tok in code.split_whitespace() {
                let kinded = if let Some(c) = tok.strip_prefix("syscall.") {
                    Some((CallKind::SysCall, c))
                } else if let Some(c) = tok.strip_prefix("call.") {
                    Some((CallKind::Call, c))
                } else {
                    tok.strip_prefix("exec.").map(|c| (CallKind::Exec, c))
                };
                if let Some((kind, callee)) = kinded {
                    let bare = callee.rsplit("::").next().unwrap_or(callee);
                    graph
                        .get_mut(parent)
                        .unwrap()
                        .insert(bare.to_string(), kind);
                }
            }
        }
    }
    graph
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

/// Parse the MASM call-graph: for each procedure body (and the implicit
/// `#main` begin block), record the set of procedures it invokes via
/// `exec.X`, `call.X`, or `syscall.X`.
///
/// This is used by the recorder's call/return detection to disambiguate
/// "deeper call" (current logic) from "sibling-after-return" — when a
/// context transition `prev → ctx` happens with the current call stack's
/// top (the caller) known to invoke BOTH `prev` and `ctx`, it must be a
/// sibling transition (the assembler skipped the intermediate return-to-
/// parent boundary).  Without this signal the recorder would push `prev`
/// onto the call stack and emit a `register_call(ctx)`, mis-nesting the
/// trace.  See `test_control_flow_call_exit_strict_lifo`.
///
/// Procedure names are stored as bare identifiers (without the
/// `#exec::` prefix the runtime adds); callers must strip that prefix
/// before looking up.
fn parse_call_graph(source: &str) -> HashMap<String, HashSet<String>> {
    let mut graph: HashMap<String, HashSet<String>> = HashMap::new();
    // Tracks the currently-open procedure body.  `#main` is the implicit
    // body of the `begin ... end` block (matches the runtime's
    // `#exec::#main` context name minus the prefix).
    let mut current: Option<String> = None;
    for line in source.lines() {
        let trimmed = line.trim();
        // Strip a trailing comment so `exec.foo # comment` still parses.
        let code = match trimmed.find('#') {
            Some(idx) => &trimmed[..idx],
            None => trimmed,
        };
        let code = code.trim();
        if code.is_empty() {
            continue;
        }
        if code.starts_with("proc.") || code.starts_with("export.") {
            // Format: proc.name.N or export.name.N
            if let Some(name) = code.split('.').nth(1) {
                current = Some(name.to_string());
                graph.entry(name.to_string()).or_default();
            }
        } else if code == "begin" {
            current = Some("#main".to_string());
            graph.entry("#main".to_string()).or_default();
        } else if code == "end" {
            current = None;
        } else if let Some(parent) = current.as_ref() {
            // Tokenise on whitespace and look for `exec.X`, `call.X` or
            // `syscall.X`.  Each token may be followed by `::path::name`
            // for namespaced calls — keep just the final segment which
            // is what the runtime surfaces as the bare context name.
            for tok in code.split_whitespace() {
                let callee = tok
                    .strip_prefix("exec.")
                    .or_else(|| tok.strip_prefix("call."))
                    .or_else(|| tok.strip_prefix("syscall."));
                if let Some(callee) = callee {
                    let bare = callee.rsplit("::").next().unwrap_or(callee);
                    graph.get_mut(parent).unwrap().insert(bare.to_string());
                }
            }
        }
    }
    graph
}

/// Strip the runtime's `#exec::` prefix from a context name so it can be
/// looked up in the source-parsed call graph.
fn bare_proc_name(context_name: &str) -> &str {
    context_name.rsplit("::").next().unwrap_or(context_name)
}

/// Invert a call graph into a parent map: child -> parent (the unique
/// procedure that invokes `child` via `exec.child`).  Returns `None`
/// for the entry's parent if the child has no caller in the source, or
/// if it has multiple callers (in which case the static chain is
/// ambiguous and we skip the chain-synthesis).
fn build_parent_map(
    call_graph: &HashMap<String, HashSet<String>>,
) -> HashMap<String, Option<String>> {
    let mut parents: HashMap<String, Vec<String>> = HashMap::new();
    for (caller, callees) in call_graph {
        for callee in callees {
            parents
                .entry(callee.clone())
                .or_default()
                .push(caller.clone());
        }
    }
    let mut result = HashMap::new();
    for (child, callers) in parents {
        if callers.len() == 1 {
            result.insert(child, Some(callers.into_iter().next().unwrap()));
        } else {
            result.insert(child, None);
        }
    }
    result
}

/// Walk the parent chain back to `#main` from a given procedure.
/// Returns the chain in OUTERMOST-FIRST order, e.g. for nested_calls
/// `chain_from_main("inner") = ["#main", "compute", "outer", "middle",
/// "inner"]`.  Returns `None` when the chain is ambiguous (a child has
/// multiple callers) or doesn't reach `#main` — the recorder then
/// falls back to its observation-driven path.
fn chain_from_main(parents: &HashMap<String, Option<String>>, leaf: &str) -> Option<Vec<String>> {
    let mut chain = vec![leaf.to_string()];
    let mut current = leaf.to_string();
    // Bound the loop so a malformed source can't make us spin forever.
    for _ in 0..256 {
        match parents.get(&current) {
            Some(Some(parent)) => {
                if parent == "#main" {
                    chain.push(parent.clone());
                    chain.reverse();
                    return Some(chain);
                }
                chain.push(parent.clone());
                current = parent.clone();
            }
            _ => return None,
        }
    }
    None
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
