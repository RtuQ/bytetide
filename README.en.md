# ByteTide

[简体中文](./README.md) | [English](./README.en.md)

A serial / network log debugging workbench for embedded development. Every line a device emits over UART, TCP, or UDP can be viewed live, searched, filtered, plotted as a waveform, and handed to an AI assistant for protocol analysis through the built-in REST bridge.

Built with Tauri 2 + Vue 3 + Rust: small binaries, fast startup, low memory footprint. All features run locally with no cloud dependency. Formerly known as Serial Tool.

## Preview

<p align="center">
  <img src="docs/preview-light.png" alt="ByteTide light theme" width="49%" />
  <img src="docs/preview-dark.png" alt="ByteTide dark theme" width="49%" />
</p>

---

## Typical use cases

- **Sensor / module debugging**: when a device periodically outputs values like `V: 220, C: 10, P: 200`, configure the frame format once and read it as a live multi-channel waveform; hover any point for its arrival time, raw frame, and decoded channel values.
- **Protocol analysis**: expose live logs to an AI CLI through the built-in REST bridge (local HTTP service with token auth); candidate headers, trailers, checksums, and frame lengths are computed server-side to assist protocol reverse engineering.
- **Soak-test monitoring**: define alert rules on keywords such as `ERROR` or `ASSERT`; each hit raises a system notification (optional beep), with count aggregation and cooldown suppression — no need to watch the screen.
- **Two-device comparison**: pair two sessions by timestamp within a tolerance, view them side by side with differences highlighted and over-tolerance deltas in red; jump between sides in either direction.
- **Offline analysis**: logs are recorded to disk as TSV. Reopening a `.log` file creates an offline session with every feature available except live I/O.

## Quick start

1. **Install**: grab a bundle from [GitHub Releases](https://github.com/RtuQ/bytetide/releases), or build from source (see [Building](#building)):
   ```bash
   npm install
   npm run tauri build   # bundle in target/release/bundle/
   ```
2. **Connect**: pick a transport in the top bar (serial / TCP client / TCP server / UDP listener), set the parameters, and hit Connect — logs start scrolling.
3. **Everyday operations**: search for keywords; select a line and press `Ctrl+B` to bookmark it; configure the frame format in the Plot panel for waveforms; enable the REST bridge when an AI needs access.

> Without hardware, load the bundled sample file [sample-data/plot-demo.log](./sample-data/plot-demo.log) (600 frames, dual-channel sine waves) via "Open Log" to try every feature except live I/O.

---

## Features

### Connections

- **Transports**: serial (COM), TCP client / TCP server / UDP listener — all with the same UX
- **Multiple tabs**: run several sessions concurrently; Split View shows 2–4 of them side by side
- **Connection control**: Stop (keep the log and tab), Reconnect (same config; logs, searches, and alerts carry over), Close (disconnect and discard the tab)
- Save connection parameters as **presets** and reapply with one click

### Reading logs

- Virtualized rendering with an in-memory buffer of the **last 50,000 lines**; very long lines scroll horizontally
- View toggles: follow tail, matches only, HEX view, inter-line delta, line numbers, RX/TX direction column
- Live RX/TX byte counts and throughput in the status bar

### Search and filtering

- **Search**: keywords or regex; the hit list sits below the search box — click to jump
- **Filter chain**: stack include/exclude stages, equivalent to `grep | grep -v`
- **Keyword highlights**: multiple keywords, each with its own color and live counter, independent from search

### Sending

- ASCII / HEX modes, `Ctrl+Enter` to send, optional trailing newline
- **Scheduled repeat sending**; click history entries to refill the input

### Automation

- **Auto-reply**: send a response whenever a rule matches — suited to automated testing of query/response protocols; rules are evaluated on the Rust side and do not depend on the UI staying alive
- **Alerts**: system notifications (optional beep) on keyword/regex hits, with windowed count aggregation and cooldown suppression; the last 100 events are kept and clickable to jump back
- **Line bookmarks**: `Ctrl+F2` / `Ctrl+B`, managed from the sidebar
- **Config presets**: save filter chains / keywords / auto-replies / frame formats as named presets; import and export the whole library as JSON for team sharing

### Monitoring

- 60-second line-rate bars, byte-rate curve, and inter-line gap min / avg / p95 / max

### Session compare

- Pair two logs by timestamp within a tolerance and view them side by side; differences are highlighted, over-tolerance deltas marked in red, and either side can be clicked to jump

### Plotting

Slice the incoming byte stream into frames and plot multi-channel values in real time.

Configurable frame format: header / trailer, checksum (none / sum / XOR — single byte over the data segment), channel count (1–16), bytes per channel (1 / 2 / 4), endianness, signedness.

```
[HEADER][DATA: channels × bytesPerChannel][CHECKSUM?][TRAILER?]
```

Example: frame `01 00 01 02 12 43`, header `01 00`, 2 channels × 2 bytes, big-endian →
Ch0 = `0x0102` = **258**, Ch1 = `0x1243` = **4675**. Hover any point for its arrival time, raw frame, and per-channel values.

Two input modes: compact binary frames (raw mode), or hex *text* such as `"01 00 12 43"` (ASCII-hex mode).

### REST analysis bridge

Enable "REST Bridge" in the sidebar and the app starts a local HTTP service (axum, Bearer token auth, bound to `127.0.0.1:8765` by default, `0.0.0.0` optional for remote access; `/health` is public). External tools — e.g. an AI CLI — can then access session data through these endpoints:

| Endpoint | Description |
|---|---|
| `GET /health` | Service status, version, buffer capacity |
| `GET /ports` | Enumerate serial ports |
| `GET /sessions`, `GET /sessions/:id` | Session list / detail (config, status, stats, log path) |
| `GET /sessions/:id/lines` | Read logs: select by line no / range / `last` / `sinceNo` / `around`, paginated, with `format=csv\|tsv` output |
| `GET /sessions/:id/follow` | Long-poll incremental reads (`sinceNo` cursor, controllable timeout and batch cap) |
| `GET /sessions/:id/stats` | RX/TX line and byte counters |
| `GET /sessions/:id/histogram` | Line counts per time bucket |
| `GET /sessions/:id/timing` | Inter-line gap stats (min / avg / p95 / max, with over-threshold gaps located individually) |
| `GET /sessions/:id/infer` | Frame-format inference: header / trailer candidates, checksum, suggested frame length |
| `GET /sessions/:id/decode` | Decode value sequences with the frame format (panel config or parameter overrides) |
| `GET /sessions/:id/value-hist` | Per-channel value distribution |
| `GET /sessions/:id/bookmarks`, `/alerts` | UI bookmarks and alert history (read-only mirrors) |
| `GET/POST/DELETE /sessions/:id/annotations` | AI annotations (idempotent merge); appear in the UI in real time |
| `GET/POST /sessions/:id/plot-config` | Read / write back the plot config; write-backs take effect in the UI immediately |
| `GET /sessions/:id/export` | Stream the complete on-disk log (including lines evicted from the in-memory buffer) |
| `POST /sessions/:id/send`, `/exchange` | Send / send-and-await-reply (off by default; enable allowSend separately in the panel) |

All read endpoints share one server-side filter set: `dir` (rx/tx), `q` (substring, comma-separated), `re` (regex), `hex` / `mask` (hex bytes / `?` wildcard mask), `exclude`, `sinceMs` / `untilMs` (time window).

Typical workflow: enable the bridge → hand the URL + token shown in the panel to the AI → the AI reads the log, infers the frame format via `/infer`, verifies it via `/decode`, then writes the confirmed config back to `/plot-config`, which the UI adopts instantly.

A ready-made AI skill package lives at [skills/serial-tool-bridge](./skills/serial-tool-bridge) (standard SKILL.md format): copy the folder into your AI assistant's skills directory — `~/.zcode/skills/` for ZCode; Claude Code / Codex work the same way.

---

## CLI (`bytetide`): headless monitoring

For hosts where a desktop environment is unavailable — servers, Raspberry Pi / ARM boards. Both desktop `v*` releases and the CLI-only `cli-v*` tags ship **static musl builds** for aarch64 / x86_64 as tarballs: a single file with no runtime dependencies. Build locally with `cargo build -p bytetide-cli --release`.

```bash
bytetide list                                  # list serial ports
bytetide monitor                               # no source arg: arrow-key port picker
bytetide monitor -p /dev/ttyUSB0 --baud 115200 --ts
bytetide monitor --tcp 192.168.1.50:9000 --retry 5
bytetide monitor --tcp-listen 9000 --json      # one JSON object per line, script-friendly
bytetide monitor --udp 5140
```

Interactive sending: type a line and press Enter to send it as ASCII; `/hex AA 01` sends hex, `/mode ascii|hex` switches modes, `/quit` exits.

```
$ bytetide monitor
? Select port › /dev/ttyUSB0 (USB Serial)
RX  [Boot] sensor-fw v2.1
TX  get temp           ← typed + Enter
RX  T=23.4C
TX  AA 01              ← /hex AA 01
^C
Disconnected: 00:03:12 · RX 1284 lines / 54.2 KB · TX 2 lines / 12 B · recorded log.tsv
```

Data lines go to stdout (`RX  ` green / `TX  ` amber, two-space columns); the connect banner and the Ctrl-C summary go to stderr — `| head` and redirections are safe (exit codes 0/1/2). `--no-color` or `NO_COLOR` disables color. `-o/--record <path|template>` records a desktop-compatible TSV that can be reopened via "Open Log" for full offline analysis; by default nothing is recorded and no timestamps are shown. See `bytetide --help` for the full reference.

---

## Building

```bash
npm install

npm run tauri dev      # dev mode: desktop app with hot reload
npm run tauri build    # installer bundle in target/release/bundle/
npm run build          # frontend only: type-check + production build
npm test               # frontend unit tests (40+)
cargo test             # Rust unit tests (run at repo root: core / cli / src-tauri)
cargo run -p bytetide-cli -- --help   # try the CLI locally
```

Requirements: Node.js 18+, Rust stable, plus the [Tauri 2 system dependencies](https://tauri.app/start/prerequisites/).
The Rust side is a Cargo workspace (`crates/bytetide-core` / `crates/bytetide-cli` / `src-tauri`): run cargo commands from the repo root; the lockfile and build artifacts live in the root `target/`.
GitHub Actions runs the full check suite on push / PR.

> `npm run dev` alone (without the Tauri backend) still renders the UI as a smoke check — serial I/O is unavailable.

---

## Project layout

```
bytetide/
├─ crates/               # Cargo workspace members
│  ├─ bytetide-core/     # Core logic: serial / sessions / ring buffer / rules / recording (Tauri-free)
│  └─ bytetide-cli/      # The bytetide CLI (headless monitoring)
├─ src/                  # Frontend (Vue 3 + TypeScript + Pinia)
│  ├─ components/        # UI: log view, send panel, search, plot, sidebar panels
│  ├─ composables/       # Headless logic: event intake, parsers, plotting, alert beep…
│  ├─ stores/            # Session state machine, alert history
│  └─ types/             # Types & defaults (single source of truth)
├─ src-tauri/            # Desktop Rust shell: invoke commands, REST bridge, hotplug, event forwarding
├─ sample-data/          # Demo log
├─ docs/                 # Screenshots & docs assets
├─ skills/               # AI skill package for the REST bridge (copy to ~/.zcode/skills/)
└─ AGENTS.md             # Code conventions (read before hacking)
```

## License

MIT — see [LICENSE](./LICENSE).
