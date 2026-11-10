# hft-lab

[![ci](https://github.com/Zerit0/htf-lab/actions/workflows/ci.yml/badge.svg)](https://github.com/Zerit0/htf-lab/actions/workflows/ci.yml)

A deterministic, event-sourced trading research stack in Rust for crypto perpetual DEXs. Hyperliquid first, Lighter next.

Every byte that crosses the process boundary is written to an append-only log. A pure state machine consumes that log. Running the state machine live and replaying the log afterwards must produce bit-identical output, and the tooling checks that it does.

> **Status: phase F0 (capture and replay).** The stack records public market data and rebuilds order books. It places no orders and needs no keys or funds.

## Why it is built this way

Trading bugs are hard to reproduce because the market never repeats itself. This stack makes every session reproducible:

```
      EDGES (non-deterministic)                CORE (deterministic)
┌────────────────────────────────────┐    ┌──────────────────────────────┐
│ network ──► session::run ──► LOG ──┼───►│ replay::run ──► sm.update()  │──► Action
│ clock          (network thread)    │tail│     (state machine thread)   │
└────────────────────────────────────┘    └──────────────────────────────┘
                                    LOG ──► hft-replay  (offline, same code)
```

- **Edges** own sockets and the clock. They log inbound events as soon as they are seen and outbound events before they are sent.
- **The core** never reads the clock, the network or the disk. Its notion of "now" is the timestamp of the event it is processing.
- **Replay** is the live code path with the network thread removed, so a production session can be re-run on a laptop, line by line.

The design follows the *How to HFT* thread by [@yenwod_](https://x.com/yenwod_): packets in, state machine, packets out, with non-determinism pushed to the edges.

## What is in the box

| Crate | Purpose | Dependencies |
|---|---|---|
| [`hft-core`](crates/hft-core) | Event model, binary log (writer and tailing reader), JSON parser, fixed-point decimals, L2 order book, Hyperliquid decoder, state machine, replay driver, WebSocket session loop | None (std only) |
| [`hft-replay`](crates/hft-replay) | Offline CLI: `run`, `verify`, `dump`, `stats`, `gen` | `hft-core` |
| [`hft-recorder`](crates/hft-recorder) | Network edge: connects over WebSocket and records all traffic | `hft-core`, `tungstenite` (WebSocket framing and TLS only) |

Everything that will eventually decide money lives in `hft-core`, which has zero external dependencies.

## Verification status

| Component | Status |
|---|---|
| `hft-core`, `hft-replay` | 33 unit tests pass. `clippy -D warnings` and `rustfmt` clean. Exercised end to end on synthetic logs of up to 200,000 messages |
| Live tail vs replay equivalence | Covered by a test: one thread writes a log while another follows it, and the live output must equal the later replay |
| Cross-platform determinism | CI produces the same digest (`ef5ec7debecdd701`) for the same synthetic log on Linux ARM64 and macOS ARM64 |
| `hft-recorder` | Compiles and passes clippy in CI against the real `tungstenite` crate. **Never run against the live exchange** |
| Hyperliquid message formats | Written from the public docs. **Not validated against the live exchange** |
| `scripts/`, `deploy/` | Shell syntax checked. **Never run on a real server** |

The quick start below closes the open items in about fifteen minutes.

## Quick start

```bash
# 1. Build and test everything
cargo test && cargo build --release

# 2. Exercise the pipeline without a network
make synth
make verify LOG=data/synth.hftlog

# 3. Record the live Hyperliquid feed for a couple of minutes (Ctrl-C to stop)
make record COINS=BTC,ETH

# 4. Validate what was recorded
make stats      # event counts by type
make verify     # must report a stable digest and "malformed 0"
make replay | head -20
```

Reading the output of step 4:

- **`malformed` above zero** means the live format differs from what `venue/hyperliquid.rs` expects. Inspect raw messages with `hft-replay dump <log> --kind ws_in --limit 5` and adjust the decoder.
- **`lag feed (ms)`** is your local clock minus the exchange timestamp. It measures feed latency from wherever the recorder runs.
- **`digest`** must be identical every time you verify the same file, on any machine.

## The CLI

CLI messages are currently in Spanish. The output below is real, from `hft-replay gen data/demo.hftlog --events 50000 --seed 1`.

```console
$ hft-replay verify data/demo.hftlog
eventos        51047
duracion       1048.1 s
libros         40071 actualizaciones, 36514 cambios de BBO
trades         9929
conexiones     2 abiertas, 2 cerradas
timers         1034
acciones       0
lag feed (ms)  ultimo 210, maximo 239
anomalias      malformed 0, cruzados 0, desordenados 0
digest         ef5ec7debecdd701
DETERMINISTA OK  (dos ejecuciones, mismo digest)
```

```console
$ hft-replay run data/demo.hftlog | head -4
1 1794300000000000000 CONN HL c1 OPEN wss://synthetic/ws
6 1794300000229834774 BBO HL ETH bid=3499.4@9.3 ask=3500.4@4.46 mid=3499.9 spread_bps=2.85 lag_ms=210
7 1794300000243975856 BBO HL BTC bid=102999.8@3.72 ask=103000.8@8.61 mid=103000.3 spread_bps=0.09 lag_ms=196
8 1794300000280817298 TRADE HL ETH BUY 0.907@3500.6 tid=1
```

| Command | What it does |
|---|---|
| `hft-replay run <log> [--quiet]` | Runs the state machine over a log and prints its output plus a summary |
| `hft-replay verify <log>` | Runs twice and compares digests. Exit code 3 if they differ |
| `hft-replay dump <log> [--limit N] [--kind K]` | Prints raw events |
| `hft-replay stats <log>` | Event and byte counts by type, duration, rate |
| `hft-replay gen <log> [--events N] [--seed S]` | Writes a synthetic session. Same seed, same file, on any machine |
| `hft-recorder --coins BTC,ETH --out-dir data` | Records L2 book and trades. `--testnet`, `--no-sm` and `--quiet` are also available |

The synthetic generator produces Hyperliquid-shaped messages for testing. It is not a market model.

## Event log format

One file per recorder session, little-endian, 8-byte header `HFTLOG01`, then records:

| Field | Size | Notes |
|---|---|---|
| `len` | u32 | Bytes following this field |
| `seq` | u64 | Gapless sequence from 0 |
| `ts_ns` | u64 | Local Unix time in nanoseconds |
| `kind` | u8 | `CONN_OPEN`, `CONN_CLOSE`, `HTTP_REQ`, `HTTP_RESP`, `WS_IN`, `WS_OUT`, `TIMER`, ... |
| `venue` | u8 | 1 Hyperliquid, 2 Lighter |
| `conn` | u16 | Connection id within the session |
| `payload` | variable | Raw bytes exactly as sent or received |
| `checksum` | u32 | FNV-1a over `seq` through `payload` |

- **One `write` call per record**, so a tailing reader can only ever see a partial record at the very end, where it simply waits.
- **Crash tolerant**: a power cut leaves at most one truncated record, which readers ignore.
- **Raw payloads**: logs stay re-processable when a decoder changes.

## Determinism rules

| Rule | Reason |
|---|---|
| Time comes from the event, never from the system clock | The wall clock is different during replay |
| `BTreeMap`, never `HashMap` | `HashMap` iteration order changes between runs |
| Prices and sizes are fixed-point `i64` (scale 1e8), never `f64` | Float rounding depends on operation order |
| No randomness without an explicit, logged seed | Same seed + capture + strategy must give the same result |
| The state machine performs no I/O | I/O belongs to the edges |

CI runs the suite on Linux ARM64 and macOS ARM64 and prints the digest of the same synthetic log on both.

## Repository layout

```
crates/hft-core/src
  event.rs      event kinds and Action (Place/Cancel reserved for F2)
  log.rs        binary log format, writer, tailing reader
  sm.rs         the state machine: pure and deterministic
  replay.rs     log consumer shared by live mode and replay
  session.rs    WebSocket session loop, generic over the transport
  book.rs       L2 book, mid, spread, taker fill price
  fixed.rs      fixed-point decimals
  json.rs       minimal JSON parser
  venue/        per-exchange decoders
crates/hft-replay     offline CLI
crates/hft-recorder   network edge
deploy/               systemd unit
scripts/              server bootstrap, log sync
docs/                 architecture, roadmap, runbook (in Spanish)
```

## Roadmap

Each phase ends with a measurable go/no-go gate. Details in [`docs/ROADMAP.md`](docs/ROADMAP.md).

| Phase | Scope | Trades? |
|---|---|---|
| **F0** | Record Hyperliquid, deterministic replay | No |
| **F1** | Add Lighter, offline study of cross-venue opportunities net of fees | No |
| **F2** | Order placement, non-blocking reactor, kill switch and risk limits | Minimal size |
| **F3** | Naive market making, maker on one venue hedged as taker on the other | Small size |
| **F4** | Simulator with seeded randomness, signal research | Scaled only if F3 holds up |

Known F0 limitations, such as the blocking connect and the file-polling queue between threads, are listed in [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) with the phase that removes each one.

## Security

- No keys, seeds or `.pem` files belong in this repository. `.gitignore` covers the common cases, but check `git status` before every commit.
- F0 and F1 use no trading credentials at all.
- From F2 on: API wallets without withdrawal permission, stored only on the server.

## Disclaimer

This is a personal research project, provided as is, with no warranty. Trading leveraged derivatives can lose all deposited capital. Nothing here is financial advice.
