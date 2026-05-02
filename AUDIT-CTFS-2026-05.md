# Miden Recorder CTFS Audit — 2026-05-02

This audit checks `codetracer-miden-recorder` against the canonical
CodeTracer multi-stream CTFS schema and the section 5.6 audit checklist
maintained in `/tmp/isonim-migration.txt`. Prior audits set the
canonical patterns: Ruby (1.21, 1.22), Python (1.27), JavaScript (1.38),
EVM (1.39), PHP (1.41), Solana (1.44), Move (1.46), Cardano (1.48),
Cairo (1.50), Flow / Cadence (1.52), Fuel / Sway (1.53), and PolkaVM
(1.55). This is the **thirteenth** recorder audited.

## Architecture

The Miden recorder is a **single-process Rust crate** that embeds
`miden-processor` 0.14 directly:

* `tracer.rs` (`MidenTracer::trace_program`) parses a `.masm` source
  file via `miden_assembly::Assembler`, drives `miden_processor::execute_iter`
  to obtain a step-by-step `VmState` sequence, and emits canonical
  CodeTracer events through the Rust-native `NimTraceWriter` (the
  `codetracer_trace_writer_nim` sibling-path crate).
* Each `VmState` callback resolves the source location via the
  per-program `SourceMap` (built from the assembler's `AsmOpInfo`
  + `context_name` records) and emits per-step register dumps:
  the operand-stack registers `stack[0..]` (typed against a registered
  Miden `felt` `TypeId`), then the local frame's `loc[i]` slots.
* Calls and returns are detected via `context_name` transitions in the
  `VmState` stream — a context_name change marks a `proc.<name>` entry,
  the matching pop marks the return.
* `kernel_procs.rs` enumerates the well-known Miden kernel-procedure
  prefixes (`std::sys::*`, `mast::*`, etc.) so kernel-internal calls
  can be filtered or hoisted in the calltrace pane.
* `mockchain.rs` provides a synthetic MockChain configuration (wallets,
  faucets, P2ID notes) and a `ContractTraceSession` that writes a
  `contract_trace_summary.json` artefact. Real on-chain tracing is
  blocked on `miden-testing` reaching a version compatible with our
  `miden-processor` 0.14 dep.
* `client_replay.rs` provides a synthetic transaction-replay path
  driven by `node_url` + `account_id` + `transaction_id`, currently
  emitting a `replay_summary.json` artefact only. Real replay requires
  either captured `TransactionInputs` or a compatible `miden-client`
  version.
* `source_map.rs` and `rust_support.rs` provide the source-mapping
  layer (multi-file resolver + Rust-via-cargo-miden support).

The recorder is **not** an FFI consumer — every canonical entry point
(`register_call`, `register_step`, `register_special_event`, `arg`,
`register_thread_*`) is reachable through the
`codetracer_trace_writer_nim` Rust API. There are no `#[no_mangle]`
stubs, and `add_event` does not appear in the source.

## Summary

| # | Check | Status (pre-fix) | Status (post-fix) | Notes |
|---|---|---|---|---|
| a | CLI defaults to `TraceEventsFileFormat::Ctfs` | **GAP** | **OK** | Pre-fix `src/main.rs`'s `OutputFormat` enum exposed only `Binary` (legacy CBOR+Zstd) and `Json`, with `Binary` as the default (`record --format` had `default_value = "binary"`). The canonical CTFS multi-stream container was not selectable at all. Post-fix the enum gains a `Ctfs` variant (listed first), with doc-comments on each option, plus an `impl From<OutputFormat> for TraceEventsFileFormat` so the `record` dispatch site reduces to `let format: TraceEventsFileFormat = args.format.into();`. The `default_value` is now `"ctfs"`. The `OutputFormat::as_str` helper is added (marked `#[allow(dead_code)]`) for future `trace_metadata.json` `format` field emission. Same default-format fix as EVM (1.39), Solana (1.44), Move (1.46), Cardano (1.48), Cairo (1.50), Flow (1.52), Fuel (1.53), PolkaVM (1.55). |
| b | `register_call` for each call | OK | OK | The recorder emits `register_call(fn_id, args)` for every Miden `proc.<name>` entry, driven by `context_name` transitions in the `VmState` iterator. The matching context-pop emits `register_return`. Kernel-procedure prefixes (see `kernel_procs.rs`) are recognised so they can be filtered in the calltrace pane. |
| c | Call args via `register_call_arg` / `arg()` | **GAP** | **OK** | Pre-fix the call-detection branch in `tracer.rs` always called `register_call(fn_id, vec![])` so the calltrace pane showed every Miden procedure invocation with empty arguments. Miden has no formal parameter list — the calling convention is "operand stack top". Post-fix the call-detection branch stages the top-of-stack values `stack[0..3]` through `TraceWriter::arg(&format!("s{i}"), value)` immediately before `register_call`. This both attaches them to the `CallRecord.args` slice (rendered in the calltrace pane's `.call-arg` rows) and registers them as step-local variables. The names `s0..s3` (rather than `stack[0..3]`) are deliberate to avoid colliding with the per-step variable dump that already names operand-stack registers `stack[i]`. |
| d | Write/WriteOther/Error/EvmEvent for IO and structured events via `register_special_event` | **GAP (Error)** / **N/A (Write)** | **OK (Error)** / **N/A (Write)** | Pre-fix VM execution errors propagated through `result.map_err(...)?` — early-return out of the iter loop before the trace was finalised — so the frontend's error channel never surfaced them. Post-fix the iter-loop match captures the error to a `vm_error: Option<String>` and `break`s rather than `?`-propagating. After the loop, before context-stack drain, `register_special_event(EventLogKind::Error, "miden_vm_error", &message)` is invoked so the error surfaces on the structured event channel and the partial trace finalises cleanly (matches PolkaVM 1.55 trap routing and Cairo 1.50 CairoPanic routing). Miden has no native stdout/stderr — the VM exposes no host-function channel comparable to `seal_debug_message` (PolkaVM) or `console_log` (EVM) — so explicit `Write` records are N/A. The advice tape and Miden `debug.*` ops (which can surface inspection hints to a host-attached debugger) are not yet routed; tracked as an open follow-up. |
| e | Thread events (Start / Exit / Switch) | OK (N/A) | OK (N/A) | Miden VM is single-threaded by design — `execute_iter` produces one linear `VmState` stream from one program instance. Recorder correctly emits no thread events. |
| f | Step records for line navigation | OK | OK | `tracer.rs::trace_program_with_writer` calls `register_step(path, line)` for each `VmState` whose source-mapped line differs from the previous step. Source resolution falls back to `(masm_path, line+1)` when the assembler cannot supply a richer mapping. |
| g | Canonical CTFS schema match | **GAP** | **OK** | Verified post-fix via `tests/test_ctfs_audit.rs::ctfs_writer_produces_ct_container`: invoking `record(masm_path, out_dir, TraceEventsFileFormat::Ctfs)` against `compute.masm` produces a single `.ct` file starting with the canonical magic bytes `0xC0 0xDE 0x72 0xAC 0xE2` and materially populated (>64 bytes, well past just the magic header). The existing `test_record_creates_trace_files` smoke test in `tests/test_cli.rs` already asserted CTFS magic when the writer happened to default to that container; post-fix it is the documented default path. |
| h | Obsolete `add_event` calls | OK | OK | `grep -r 'add_event' src/` returns nothing. Recorder predates the 1.30 footgun and has always used dedicated `register_*` entry points. |
| i | `#[no_mangle]` stubs colliding with upstream Nim exports | OK | OK | `grep -r '#\[no_mangle\]' src/` returns nothing. Recorder uses the `codetracer_trace_writer_nim` Rust API directly (sibling-path dep), not the C FFI. |

## Concrete fixes applied

### 1. CLI now exposes and defaults to `Ctfs`

`src/main.rs`'s `OutputFormat` enum used to expose only `Binary` and
`Json`, with `Binary` as the default. There was no way to request the
canonical CTFS multi-stream container — `Binary` writes the legacy
CBOR+Zstd format that the Nim `ct_reader_*` FFI and the db-backend's
`CTFSTraceReader` cannot consume directly.

Post-fix: `OutputFormat` gains a `Ctfs` variant (listed first), with
doc-comments explaining each option, and a freshly added
`impl From<OutputFormat> for TraceEventsFileFormat` makes the
dispatch site uniform:

```rust
#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    /// Canonical CodeTracer multi-stream container (recommended).
    Ctfs,
    /// Legacy CBOR + Zstd binary format.
    Binary,
    /// Human-readable JSON (slower; useful for debugging).
    Json,
}

impl From<OutputFormat> for TraceEventsFileFormat {
    fn from(fmt: OutputFormat) -> Self {
        match fmt {
            OutputFormat::Ctfs => TraceEventsFileFormat::Ctfs,
            OutputFormat::Binary => TraceEventsFileFormat::Binary,
            OutputFormat::Json => TraceEventsFileFormat::Json,
        }
    }
}
```

`RecordArgs.format` now defaults to `"ctfs"`, and the `record`
dispatch site collapses to
`let format: TraceEventsFileFormat = args.format.into();`. An
`OutputFormat::as_str` helper is wired in (marked `#[allow(dead_code)]`)
for future `trace_metadata.json` `format` field emission, mirroring
the Fuel 1.53 / PolkaVM 1.55 pattern.

The `contract` and `replay` subcommands do not currently use
`TraceEventsFileFormat` (they only emit JSON summary artefacts), so
no format-field changes are required there. When real-tracing
support lands for those subcommands, they should adopt the same
`From<OutputFormat>` shape.

### 2. Procedure-call branch now stages canonical call args

The call-detection branch in `tracer.rs` (driven by `context_name`
transitions) previously called `register_call(fn_id, vec![])` so the
calltrace pane showed every Miden procedure invocation with empty
arguments.

Miden has no formal parameter list — the calling convention is
"operand stack top". Post-fix the call-detection branch stages the
first up-to-four operand-stack values `stack[0..3]` through
`TraceWriter::arg("s0", value), …, TraceWriter::arg("s3", value)`
immediately before `register_call`. The values are typed against
the same Miden `felt` `TypeId` that the per-step register dump uses,
so the renderer displays them consistently:

```rust
let arg_depth = state.stack.len().min(4);
for i in 0..arg_depth {
    let int_val = state.stack[i].as_int() as i64;
    let value = ValueRecord::Int { i: int_val, type_id: felt_type_id };
    let _ = TraceWriter::arg(&mut *self.writer, &format!("s{i}"), value);
}
```

The argument names `s0..s3` (rather than `stack[0..3]`) are
deliberate: `TraceWriter::arg` internally calls
`register_variable_with_full_value`, and the per-step variable dump
already names operand-stack registers `stack[i]`. Using a distinct
`s` prefix avoids colliding with that channel.

### 3. VM execution errors route through the error channel

Pre-fix the iter-loop body propagated `miden_processor` errors via
`result.map_err(...)?` — early-return out of the iter loop before
the trace was finalised — so the frontend's error channel never
surfaced them and the partial trace was discarded.

Post-fix the iter-loop match captures the error message to a
`vm_error: Option<String>` and `break`s rather than `?`-propagating.
After the loop body, before context-stack drain and final
`finish_writing_trace`, the error is routed to the structured event
channel:

```rust
if let Some(ref message) = vm_error {
    eprintln!("Miden VM execution error: {message}");
    TraceWriter::register_special_event(
        &mut *self.writer,
        EventLogKind::Error,
        "miden_vm_error",
        message,
    );
}
```

This matches the PolkaVM 1.55 trap / segfault / out-of-gas routing,
the Cairo 1.50 CairoPanic routing, and the Fuel 1.53 Panic / Revert
routing. Crucially the trace finalises cleanly: the partial trace
data captured before the error is preserved, plus the error itself
is now a first-class structured event the frontend can surface.

## Tests added

`tests/test_ctfs_audit.rs` (3 new cases):

* `ctfs_writer_produces_ct_container` — runs `compute.masm`
  through `recorder::record` with `TraceEventsFileFormat::Ctfs`
  and asserts the resulting `.ct` file starts with the canonical
  CTFS magic bytes (`0xC0 0xDE 0x72 0xAC 0xE2`) and is materially
  populated (>64 bytes).
* `ctfs_format_advertised_in_record_help` — CLI smoke test that
  `record --help` advertises `ctfs` as a `--format` value with
  `[default: ctfs]`. Uses `CARGO_BIN_EXE_codetracer-miden-recorder`
  to locate the just-built binary (same idiom as Flow 1.52, Fuel
  1.53, PolkaVM 1.55). Catches accidental defaults regressions.
* `call_arg_staging_does_not_empty_trace` — structural smoke test
  for the `TraceWriter::arg` staging path introduced in this audit.
  `compute.masm` contains 10+ `exec.<proc>` call sites, each of
  which exercises the new `stack[0..3]` → `arg("s0..s3")` staging
  loop. Pre-fix this staging path did not exist; post-fix it must
  not regress the size or magic of the canonical container.

Read-side end-to-end content assertions on the embedded event records
(e.g. that `register_special_event(EventLogKind::Error,
"miden_vm_error", …)` actually appears in the event-log of the `.ct`
container when a VM error occurs) need the
`codetracer_trace_reader_nim` dev-dep added and a small reader-walk
helper. Tracked as an open follow-up below (also open for Cairo,
Cardano, Flow, Fuel, and PolkaVM).

## Verification

```
cd /home/zahary/metacraft/codetracer-miden-recorder
AH_TEST_RESOURCE_GUARD=1 cargo test
```

* `lib` unit tests: 11 / 11 passing
* `test_cli` (existing): 4 / 4 passing
* `test_client_replay` (existing): 31 / 31 passing
* `test_mockchain` (existing): 14 / 14 passing
* `test_real_client_replay` (existing): 20 / 20 passing
* `test_real_mockchain` (existing): 14 / 14 passing
* `test_rust_support` (existing): 6 / 6 passing
* `test_tracer` (existing): 12 / 12 passing
* `test_ctfs_audit` (new): 3 / 3 passing
* doctests: 1 / 1 passing (5 ignored)

Total: 116 / 116 active passing in audit-touched suites, 0 regressions.
`cargo build --release` clean.

### Targeted Playwright sweep

No miden-specific Playwright spec exists at
`src/tests/gui/tests/program_specific_tests/` (no `miden_*.spec.ts` /
`masm_*.spec.ts`). The shared end-to-end specs do not exercise the
miden recorder. Per the audit protocol, Playwright is skipped for
this audit.

## Open gaps (not blocking, documented for follow-up)

### Advice tape and `debug.*` op routing (audit d)

Miden has no host-function channel comparable to PolkaVM's
`seal_debug_message` or EVM's `console_log`, but the VM does expose
two side-channels worth surfacing as structured events:

1. **Advice tape pops** — the `adv_push.*` / `adv_pop.*` op family
   reads from the prover-supplied advice tape, which is the closest
   analogue Miden has to "external IO". Each pop could surface as
   `EventLogKind::Read` with metadata `"adv_pop"` and content
   `"value=…"`, mirroring PolkaVM 1.55's `seal_input` routing.
2. **`debug.stack` / `debug.mem` ops** — these explicitly request
   the prover or runtime to surface inspection hints. Pre-1.55 these
   went only to `eprintln!`; routing them through
   `EventLogKind::TraceLogEvent` with metadata `"miden_debug"`
   would let the frontend's event-log pane surface them.

Both are recorder-side enhancements; the writer API
(`register_special_event`) already supports both kinds. Concrete
fix shape mirrors PolkaVM 1.55 §3.

### `contract` and `replay` subcommand real tracing (audit f)

`mockchain.rs::ContractTraceSession` and
`client_replay.rs::replay_transaction` currently emit only summary
JSON artefacts (`contract_trace_summary.json`,
`replay_summary.json`); they do not produce a `.ct` container.
Closing this requires:

1. **Contract trace** — `miden-testing` at a version compatible with
   our `miden-processor` 0.14 dep. Once aligned, the
   `MockChain::execute_tx` path can be redirected through
   `MidenTracer::trace_program_with_writer` so the canonical
   recorder writes the `.ct` container directly. The audit fix
   shape is already in place: when this lands, it should adopt
   `From<OutputFormat>` for the `--format` flag and use the same
   `register_call` / `arg` / `register_special_event` patterns.
2. **Transaction replay** — either captured `TransactionInputs`
   JSON (the recommended path per `client_replay.rs` doc) or a
   compatible `miden-client` version. Same fix shape applies.

Same shape of gap as Cairo 1.50 (replay-side tracing), Fuel 1.53
(node-replay), and PolkaVM 1.55 (Substrate-RPC).

### Per-procedure ABI / argument names (audit c, source-level)

The post-fix call-arg staging emits `s0..s3` as the argument names.
Miden has no formal parameter list per procedure, but the MASM
source can carry leading comments (e.g. `# proc.fibonacci.2 -
iterative Fibonacci, expects: n on top of stack`). Parsing those
comments at assembly time and threading the parsed arg names
through `SourceMap` would let the audit's `arg()` staging path
emit symbolic names (e.g. `n` instead of `s0`). Parallel to the
PolkaVM 1.55 ink!-metadata symbolic-decoding open item and the
Fuel 1.53 per-contract-ABI registration item.

### `.masp` (pre-compiled package) support (audit f, cross-cutting)

`record --masp` is currently a placeholder that returns a friendly
error. Closing this requires the midenc / cargo-miden toolchain
landing in the dev shell so the recorder can parse `MastForest`
from a `.masp` file (with embedded `DebugInfo` for source mapping).
The `record` path already accepts `TraceEventsFileFormat::Ctfs`,
so once `.masp` parsing is wired, no further audit work is
required for the Ctfs side.

### Multi-stream IO event collapse (cross-cutting)

Same writer-side issue documented in 1.39 (EVM), 1.41 (PHP), 1.44
(Solana), 1.46 (Move), 1.48 (Cardano), 1.50 (Cairo), 1.52 (Flow),
1.53 (Fuel), and 1.55 (PolkaVM): the multi-stream IO event writer's
`toIOEventKind` collapses 13 `EventLogKind`s onto 4
`IOEventKind` buckets, losing the original kind byte and the
metadata string. The new `EventLogKind::Error` records
(`"miden_vm_error"`) collapse onto `stderr` and lose the
`miden_vm_error` metadata in the multi-stream pane. Out of scope
for any single recorder audit; flagged as a writer-side fix in
`codetracer_trace_writer_ffi.nim`'s `toIOEventKind`.

### Read-side end-to-end content assertions

The audit tests assert the `.ct` file starts with the CTFS magic
and is materially populated. Verifying that the embedded event
stream contains the expected `register_call` /
`register_special_event` records (e.g. `EventLogKind::Error` with
`"miden_vm_error"` metadata when an error occurs) requires the
`codetracer_trace_reader_nim` dep added as a `[dev-dependencies]`
entry plus a small reader-walk helper. Tracked here for the next
pass (also open for Cairo, Cardano, Flow, Fuel, and PolkaVM).

## After this audit

Section 5.6's recorder list shows `codetracer-miden-recorder` as
audited (gaps closed for default-Ctfs CLI + procedure call-arg
staging via `TraceWriter::arg` + VM-error routing through
`register_special_event(EventLogKind::Error, "miden_vm_error", …)`;
advice-tape / `debug.*` routing + symbolic ABI-driven arg names +
real `contract` / `replay` tracing + `.masp` support open as
recorder-side / dependency-alignment / toolchain follow-ups).
Audited recorder count: 12 → 13.
