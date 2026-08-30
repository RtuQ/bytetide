---
name: serial-tool-bridge
description: Read and analyze live serial/UART logs from the ByteTide (formerly serial_tool) Tauri app via its REST bridge — decode binary frames, infer protocol grammar, inspect timing/histograms, and (when explicitly requested) probe command-response exchanges. Use when the user wants to analyze device serial output, decode a binary protocol, reverse-engineer a frame format, troubleshoot serial communication, or analyze an offline-loaded .log file.
---

# serial-tool-bridge

Connect an external AI CLI to the **ByteTide** Tauri app (formerly serial_tool)'s REST analysis bridge to read and analyze live serial logs. ByteTide is the **data source** (it owns the serial port and retains a recent ring buffer of lines *with raw bytes*); this skill is the **bridge** that lets you query it over HTTP with `curl`.

## When to use

- "Analyze the serial/UART output from my device"
- "Decode this binary protocol / figure out the frame format"
- "What values is channel X reporting over the serial link?"
- "The device isn't responding — inspect the RX/TX timing"
- "Send command `0xAA 0x01` and show me the reply" (only when the user explicitly asks to send)

## Connection (required, do this first, every session)

Two environment variables, set by the user on the AI's machine (the app prints its URL + token in the **REST 桥接** sidebar panel):

- `SERIALTOOL_URL` — base URL, e.g. `http://127.0.0.1:8765` (local) or `http://192.168.1.50:8765` (remote/VM — user picked bind `0.0.0.0`).
- `SERIALTOOL_TOKEN` — Bearer token. Empty/disabled bridge → all calls 401.

If the user pastes the URL + token directly in the chat, use those values verbatim in place of the env vars — don't wait for env setup.

**First call (smoke check):**
```bash
curl -sS -H "Authorization: Bearer $SERIALTOOL_TOKEN" "$SERIALTOOL_URL/health"
```
Expect `{"ok":true,"tokenSet":true,"allowSend":...}`. If you get 401/connection refused, STOP and tell the user to enable the bridge and copy URL+token into `SERIALTOOL_URL`/`SERIALTOOL_TOKEN`. Do not invent a URL.

All authenticated requests carry the header: `Authorization: Bearer $SERIALTOOL_TOKEN`. `/health` is the only public endpoint.

## Model (read this before querying)

- A **session** is one open serial port (`s1`, `s2`, …) or an offline-loaded `.log` file (`o1`, `o2`, …). Both are visible over REST. Offline sessions have `status="offline"`, are **read-only** (`/send` returns 400 "离线会话不可发送"), and `/follow` on an offline session always times out (no new lines ever arrive — just read `/lines`).
- Each line has a backend-assigned **`no`** (monotonic per session, **independent** of the app's UI line number, keeps growing after ring eviction). Use `no` for `?no=`, `?from=&to=`, `?sinceNo=`, `?around=`.
- The bridge keeps a **ring of the last 20,000 lines per session**, and — unlike the on-disk TSV — it **retains raw `bytes`** (so binary/0x80+ data is intact). `stats.firstNo`/`lastNo`/`size` tell you the available window. For **full history** beyond the ring, stream `/export` (on-disk TSV: `ts/dir/text` only, no bytes — see `reference/endpoints.md`).
- The app's **user annotations are visible**: `/bookmarks` (lines the user starred) and `/alerts` (fired alert history) are strong "look here" hints. Check them early in an analysis.
- **You can annotate back**: `POST /annotations` flags lines in the app UI (purple marker + sidebar panel) so the user sees what you found. Keep notes few and meaningful.
- Once you've **confirmed a grammar**, you can write it back with `POST /plot-config` so it appears live in the user's plot panel — whole replace; tell the user and prefer consent (see Safety rules).
- `bytes` is only present when the line contained invalid UTF-8; for valid UTF-8 lines, reconstruct bytes from `text` if you need them. See `reference/frame-grammar.md`.

## Standard analysis flow

1. `GET /sessions` → pick the session `id`.
2. `GET /sessions/:id/stats` → confirm data is flowing (`rxLines>0`, `firstNo`/`lastNo` window).
3. `GET /sessions/:id/bookmarks` and `/alerts` → user-flagged lines & fired alerts; inspect these first.
4. `GET /sessions/:id/lines?last=20` → eyeball recent traffic (text + bytes).
5. `GET /sessions/:id/infer?dir=rx&minRepeat=3` → candidate heads/tails/checksum/suggestedFrameLen.
6. `GET /sessions/:id/plot-config` → see what grammar the user already configured in the app (reuse it).
7. `GET /sessions/:id/decode?head=AA55&tail=&checksum=xor&channels=2&bytes=2&endian=big` → decode frames; iterate grammar until `lastError` is empty and `frameCount` is plausible.
8. Grammar confirmed → (with user consent) `POST /plot-config` to save it into the app.
9. `GET /sessions/:id/value-hist?head=AA55&...&channel=0&topN=20` → value distribution for a channel.
10. `GET /sessions/:id/histogram?bucket=1000&dir=rx` and `/timing?dir=rx` → traffic rhythm and gaps.
11. `GET /sessions/:id/lines?around=<no>&span=10` → inspect context around an interesting frame.
12. Found something the user should see → `POST /annotations` to flag those lines in the app.
13. `GET /sessions/:id/follow?sinceNo=<lastNo>&timeoutMs=5000` → wait for new live data (long-poll, 50ms granularity).
14. History older than `stats.firstNo` → `GET /sessions/:id/export` (or `?info=1` for path/size) and analyze the file locally.

Report findings: inferred grammar, decoded value sequences, anomalies (checksum mismatches, timing gaps, unexpected bytes). Always cite `no` and `rawHex` so the user can cross-check in the app.

## Safety rules (non-negotiable)

- **`allowSend` defaults to OFF.** While it's off, `/send` and `/exchange` return 403. That's the safe state — leave it.
- **Never call `/send` or `/exchange` unless the user explicitly asks to transmit.** "Analyze" / "inspect" / "decode" means **read-only**. Default to read-only even if the user is vague.
- When you *do* send, state exactly what bytes you're transmitting and why, before doing it. Prefer `/exchange` (send + capture first matching RX) over blind `/send`.
- **`POST /plot-config` is not device I/O but it mutates the user's UI** (whole replace of their plot grammar). State what you're saving; ask first unless the user asked you to save the grammar.
- **Annotations (`POST /annotations`) are user-visible too.** They're meant for findings ("this is where the checksum breaks"), not chatter — a handful of short notes per analysis, not one per line.
- This is a **LAN, plaintext** link (no TLS). For untrusted networks the user should tunnel over SSH. Don't flag this as an error — it's by design.

## Endpoint reference

Full per-endpoint params/responses/curl examples: `reference/endpoints.md`.
Frame grammar (layout, parseValue, checksum, ascii-hex, bytes rules): `reference/frame-grammar.md`.

Read those when you need exact field names, defaults, or limits. The query params are camelCase (`sinceNo`, `untilMs`, `timeoutMs`, `topN`) — see the reference.
