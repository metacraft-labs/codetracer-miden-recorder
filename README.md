## codetracer-miden-recorder

A recorder for Miden VM programs that produces [CodeTracer](https://github.com/metacraft-labs/CodeTracer) traces.

> **Note:** This project is in early development. APIs and trace formats may change.
> We welcome contributions and discussion!

### Overview

`codetracer-miden-recorder` assembles MASM source files, executes them
through the Miden VM, and captures step-level execution traces in the
canonical CodeTracer CTFS multi-stream format.  It also provides
(currently-stubbed) `contract` and `replay` subcommands for tracing
Miden contracts in a MockChain environment and for replaying on-chain
transactions.

### Building

```bash
cargo build
```

Or enter the Nix dev shell first:

```bash
nix develop
cargo build
```

### Usage

#### Record a MASM program

```bash
codetracer-miden-recorder record <file.masm> --out-dir <dir>
```

Assembles the `.masm` source in debug mode, executes it through the
Miden VM, and writes a CTFS trace bundle to `--out-dir`.

The recorder always writes traces in the canonical CodeTracer CTFS
multi-stream format (a single `.ct` container plus
`trace_metadata.json` / `trace_paths.json` sidecars).  There is no
`--format` flag — see "Converting traces" below for human-readable
output.

#### Trace a contract on a MockChain

```bash
codetracer-miden-recorder contract --wallet-id 0x1000 --faucet-id 0x2000
```

Builds a synthetic MockChain (wallets / faucets / P2ID notes) and
runs a transaction with tracing.  Currently emits a
`contract_trace_summary.json` artefact only; full CTFS contract
tracing requires `miden-testing` at a version compatible with our
`miden-processor` dependency.

#### Replay an on-chain transaction (stub)

```bash
codetracer-miden-recorder replay --node-url <url> --account-id <id> --transaction-id <tx>
```

Currently emits a `replay_summary.json` artefact only.  Real replay
requires either captured `TransactionInputs` (`--captured-inputs <path>`)
or a compatible `miden-client` version.

#### Converting traces to JSON / text

The recorder is CTFS-only.  To convert a recorded `.ct` bundle to a
human-readable form, use `ct print` from
[`codetracer-trace-format-nim`](../codetracer-trace-format-nim):

```bash
ct-print --json <recording-dir>/<program>.ct
```

`ct-print` accepts `--json`, `--json-events`, `--summary`, and
`--follow` modes; see its `--help` for details.  This conversion path
is the canonical way to produce textual oracles for golden-snapshot
tests, debugging, and interop with non-CodeTracer tools — see
`Recorder-CLI-Conventions.md` §4 in the `codetracer-specs` repo.

### Architecture

The recorder is structured around the following modules in `src/`:

| Module             | Purpose                                           |
| ------------------ | ------------------------------------------------- |
| `main.rs`          | CLI entry point (clap)                            |
| `recorder.rs`      | Top-level recording orchestration                 |
| `tracer.rs`        | Step-level trace capture during Miden VM execution |
| `source_map.rs`    | Mapping between assembler offsets and MASM source |
| `kernel_procs.rs`  | Well-known kernel procedure name prefixes         |
| `mockchain.rs`     | Synthetic MockChain configuration / sessions     |
| `client_replay.rs` | On-chain transaction replay (stub)                |
| `rust_support.rs`  | Rust-via-cargo-miden source mapping helpers       |
| `lib.rs`           | Public library API                                |

### Testing

```bash
cargo test
```

Test programs live in:

- `test-programs/masm/` — standalone MASM programs

### Environment variables

The recorder respects the standard CodeTracer recorder env-var contract
defined in `Recorder-CLI-Conventions.md` §5:

| Variable                              | CLI equivalent | Description                                                                                  |
| ------------------------------------- | -------------- | -------------------------------------------------------------------------------------------- |
| `CODETRACER_MIDEN_RECORDER_OUT_DIR`   | `--out-dir`    | Fallback output directory when `--out-dir` is omitted.  The CLI flag always wins.            |
| `CODETRACER_MIDEN_RECORDER_DISABLED`  | —              | Set to `1` or `true` to run the recorder in pass-through mode (no trace artefacts written). |
| `CODETRACER_MIDEN_RECORDER_LOG_LEVEL` | —              | Recorder log verbosity (advisory; the Miden recorder currently logs to stderr unconditionally). |

### Contributing

We'd be very happy if the community finds this useful, and if anyone wants to:

* Use and test the Miden support of CodeTracer.
* Provide feedback and discuss alternative implementation ideas: in the issue tracker, or in our [discord](https://discord.gg/qSDCAFMP).
* Contribute code to enhance the Miden support of CodeTracer.
* Provide [sponsorship](https://opencollective.com/codetracer), so we can hire dedicated full-time maintainers for this project.

### Legal info

LICENSE: Apache-2.0

Copyright (c) 2025 Metacraft Labs Ltd
