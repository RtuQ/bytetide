# Endpoints reference

Base URL: `$SERIALTOOL_URL`. Auth: `Authorization: Bearer $SERIALTOOL_TOKEN` on every route except `/health`.
List endpoints default `limit=500`, capped at `5000`. Query params are **camelCase**.
`BridgeLine = { no, ts, dir:'rx'|'tx', text, bytes?:number[]|null, epochMillis, match?:{offset,length,field:'text'|'bytes'} }`.
A missing session returns `404`; a bad param returns `400`; a missing/invalid token returns `401`; `/send`/`/exchange` while `allowSend` is off returns `403`.

## Metadata

### `GET /health` (public)
`{ ok, version, ringCap, tokenSet, allowSend }`
```bash
curl -sS "$SERIALTOOL_URL/health"
```

### `GET /ports`
`PortInfo[] = [{ name, portType, vendor?, product?, serial? }]`

### `GET /sessions`
`[{ id, config:PortConfig, status, lineCount, ringCap }]` (`status` ∈ connected|disconnected|offline). `offline` = a `.log` file loaded for analysis (id `o{N}`); read-only (`/send` → 400) and `/follow` always times out.

### `GET /sessions/:id`
`{ id, config, status, stats:BridgeStats, logPath }` (`status` ∈ connected|disconnected|offline). `offline` = a `.log` file loaded for analysis (id `o{N}`); read-only (`/send` → 400) and `/follow` always times out. `logPath` = the on-disk log file streamed by `/export`.

### `GET /sessions/:id/stats`
`{ rxLines, txLines, rxBytes, txBytes, firstNo, lastNo, firstTs, lastTs, firstEpoch, lastEpoch, ringCap, size }`
`firstNo`/`lastNo` are `0` when the ring is empty. Use them to pick `?from=`/`?to=`/`?sinceNo=`.

### `GET /sessions/:id/plot-config`
`{ enabled, source:'binary'|'ascii-hex', frameHead, frameTail, checksum:'none'|'sum'|'xor', channels, bytesPerChannel, endian:'big'|'little', signed, maxPoints }`
The grammar the user already configured in the app. `/decode` defaults to this; pass overrides.

### `POST /sessions/:id/plot-config` — write back a grammar
Body: the full PlotConfig shape above (whole replace — GET first, modify, POST back). Backend validates/normalizes (enum fields lowercased & restricted, `head`/`tail` must be hex pairs, `channels` clamped 1..16, `bytesPerChannel` ∈ 1|2|4, `maxPoints` clamped); invalid → 400 with reason. On success the stored config is returned and the app UI adopts it **live** via a `bridge-plot-updated` event (plot panel + waveform update instantly).
Not gated by `allowSend` (it never touches the device), but it **overwrites the user's plot panel** — say what you're saving and prefer consent unless the user asked to save the grammar.
```bash
curl -sS -X POST -H "Authorization: Bearer $SERIALTOOL_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"enabled":true,"source":"binary","frameHead":"AA55","frameTail":"","checksum":"xor","channels":2,"bytesPerChannel":2,"endian":"big","signed":false,"maxPoints":2000}' \
  "$SERIALTOOL_URL/sessions/s1/plot-config"
```

### `GET /sessions/:id/bookmarks`
`BridgeBookmark[] = [{ no, ts, text }]` — lines the user starred in the app (strong "look here" hints). `no` is the **app UI line number** (not the bridge `no`); `text` is a ~200-char excerpt (empty if the line was evicted from the app's buffer). To get bridge `no`s, search the text: `/lines?q=<excerpt-fragment>`.

### `GET /sessions/:id/alerts`
`BridgeAlert[] = [{ id, ruleId, pattern, level:'info'|'warn'|'err', no, ts, text, at }]` — the app's alert history (newest first, ring of 100). Same UI-line-number caveat as bookmarks.

### AI annotations — flag lines back into the app (write-in)

The symmetric counterpart to bookmarks: annotations you POST appear **live in the app UI** — annotated lines get a purple marker in the log view, and the "AI 批注" sidebar panel lists them (click to jump). The user can delete or clear them from the panel.

- `GET /sessions/:id/annotations` → `BridgeAnnotation[] = [{ id, no, ts, text, note, at }]`
- `POST /sessions/:id/annotations` — body `{"notes":[{"no":123,"note":"checksum failures start here"}]}` (`ts`/`text` optional — the backend fills them from the ring when the line is still buffered). Idempotent on `(no, note)`; ring capped at 200/session (oldest dropped). Returns `{ added, annotations }`.
- `DELETE /sessions/:id/annotations[?id=<noteId>]` — remove one, or clear all without `id`; returns the remaining list.

Etiquette: these are user-visible — keep them few, short and actionable (what you found and where to look). `no` maps 1:1 to the app's line numbers unless the user cleared the screen (screen-clear resets app numbering).

### `GET /sessions/:id/export[?info=1]`
Stream the session's **full on-disk log** as `text/plain` (chunked; safe for very large files). This is the complete history — unlike `/lines` it includes lines evicted from the 20k ring — but the format is the append-only TSV `ts\tdir\ttext` per line: **no `no`, no `epochMillis`, no raw bytes** (`text` holds U+FFFD for invalid UTF-8). `clearLog` truncates the file. Offline sessions export their source file.
`?info=1` → `{ path, sizeBytes, missing }` metadata only.

## Retrieval

### `GET /sessions/:id/lines`
**Selection** (first that applies, in priority order):
| param | meaning |
|---|---|
| `no=<int>` | the single line with that `no` |
| `from=<int>&to=<int>` | closed `[from,to]` range |
| `last=<int>` | last N lines |
| `sinceNo=<int>` | all lines with `no > sinceNo` (cursor for tail) |
| `around=<int>&span=<int>` | context window of ±span around `no` (nearest if absent); `span` default 10 |
| `from=<int>` (no `to`) | `no >= from`, then limit applies |
| *(none)* | whole filtered ring |

**Filters** (combinable, all must pass):
| param | meaning |
|---|---|
| `dir=rx|tx` | direction |
| `q=<substr>` | substring in `text`; comma-separated = multiple (all must match) |
| `ci=1` | case-insensitive for `q`/`exclude`/`re` text search |
| `re=<regex>` | regex on `text` |
| `hex=AA55` | byte substring in `bytes` (hex, whitespace ignored) |
| `mask=AA55????CS` | byte pattern with `??` = any byte |
| `exclude=<substr>` | negative: lines containing this are dropped |
| `sinceMs=<epochMs>&untilMs=<epochMs>` | time window on `epochMillis` |

**Pagination/export:** `offset=`, `limit=` (default 500, max 5000), `format=json|csv|tsv`. csv/tsv `bytes` column = space-separated uppercase hex (`AA 55 ...`), empty when the line has no raw bytes.
**Response (json):** `{ lines:BridgeLine[], total, firstNo, lastNo, size, truncated }`. `total` = matched count before paging. When filters are active, each returned line carries `match` (first hit). csv/tsv: header `no,ts,dir,text,bytes,epochMillis`.
```bash
curl -sS -H "Authorization: Bearer $SERIALTOOL_TOKEN" \
  "$SERIALTOOL_URL/sessions/s1/lines?last=20"
curl -sS -H "Authorization: Bearer $SERIALTOOL_TOKEN" \
  "$SERIALTOOL_URL/sessions/s1/lines?from=100&to=120&q=OK,ERR&hex=AA55&sinceMs=$(($(date +%s)000-60000))"
```

### `GET /sessions/:id/follow?sinceNo=<int>&timeoutMs=<int>`
Long-poll: blocks up to `timeoutMs` (default 5000, max 30000) for lines with `no > sinceNo`, polling the ring every 50ms.
`{ lines:BridgeLine[], lastNo, timedOut }`. If `timedOut` and no new data, retry with `sinceNo=<returned lastNo>`.
```bash
curl -sS -H "Authorization: Bearer $SERIALTOOL_TOKEN" \
  "$SERIALTOOL_URL/sessions/s1/follow?sinceNo=1234&timeoutMs=5000"
```

## Server-side analysis (precomputed)

**Filters are shared**: every endpoint below (`/histogram`, `/timing`, `/decode`, `/value-hist`, `/infer`) accepts the full `/lines` filter set (`dir`/`q`/`re`/`hex`/`mask`/`exclude`/`ci`/`sinceMs`/`untilMs`) alongside its own params, and defaults to `dir=rx` unless told otherwise. Example: `/decode?head=AA55&exclude=HEARTBEAT` decodes frames only from lines not containing `HEARTBEAT`.

### `GET /sessions/:id/histogram?bucket=<ms>&<filters>`
Buckets filtered lines by `epochMillis` into `bucket`-ms windows (default 1000).
`[{ bucketStart:epochMs, count }]`, sorted ascending. Accepts all `lines` filters.
```bash
curl -sS -H "Authorization: Bearer $SERIALTOOL_TOKEN" \
  "$SERIALTOOL_URL/sessions/s1/histogram?bucket=500&dir=rx"
```

### `GET /sessions/:id/timing?dir=rx&gapMs=<int>`
Adjacent-arrival gaps among filtered lines (default dir `rx`).
`{ count, minGap, maxGap, avgGap, p95Gap, gaps:[{fromNo,toNo,durationMs,fromTs,toTs}] }`.
`gaps` lists only gaps **> gapMs**; `gapMs` default = `p95Gap`.
```bash
curl -sS -H "Authorization: Bearer $SERIALTOOL_TOKEN" \
  "$SERIALTOOL_URL/sessions/s1/timing?dir=rx"
```

### `GET /sessions/:id/decode?<grammar>&from=&to=&limit=`
Decode frames using grammar (defaults to stored `plot-config`):
| param | meaning |
|---|---|
| `head` | header hex, e.g. `AA55` (whitespace ignored) |
| `tail` | tail hex |
| `checksum` | `none` \| `sum` \| `xor` |
| `channels` | channel count |
| `bytes` | bytes per channel (1/2/4) |
| `endian` | `big` \| `little` |
| `signed` | `1`/`true` |
| `source` | `binary` \| `ascii-hex` |
| `from`/`to` | restrict to `no` range |
| `limit` | max frames (default 500, max 5000) |

`{ frames:[{ no, idx, values:number[], rawHex, ts, epochMillis, valid, error? }], frameCount, lastError, scanned }`.
`no`/`ts`/`epochMillis` come from the RX line containing the frame's last byte. `lastError` holds the last checksum mismatch position. See `frame-grammar.md`.
For **offline text logs** (lines have no `bytes`), `source=binary` falls back to the `text` UTF-8 bytes - wrong if the text is hex digits like `AA55 0102`. Use `source=ascii-hex` so the hex char pairs in `text` are decoded into real bytes before framing.
```bash
curl -sS -H "Authorization: Bearer $SERIALTOOL_TOKEN" \
  "$SERIALTOOL_URL/sessions/s1/decode?head=AA55&checksum=xor&channels=2&bytes=2&endian=big"
```

### `GET /sessions/:id/value-hist?<grammar>&channel=<i>&from=&to=&topN=`
Same grammar params as `/decode`, plus:
| param | meaning |
|---|---|
| `channel` | 0-based channel index (default 0) |
| `topN` | top values by frequency (default 20) |

`{ channel, samples, distinct, min, max, mean, distribution:[{ value, count }] }`.
`distinct` = number of distinct values after quantizing to 4 decimals (not `samples`); `distribution` is top-N by frequency, so it may be shorter than `distinct`.
```bash
curl -sS -H "Authorization: Bearer $SERIALTOOL_TOKEN" \
  "$SERIALTOOL_URL/sessions/s1/value-hist?head=AA55&channels=2&bytes=2&channel=0&topN=20"
```

### `GET /sessions/:id/infer?dir=rx&minRepeat=<int>`
Heuristic candidate-frame detection on RX line bytes:
`{ heads:[{hex,count}], tails:[{hex,count}], checksums:[{kind:'sum'|'xor',count}], suggestedFrameLen?:number }`.
- `heads`: top frequent 2-byte (falling back to 1-byte) prefixes.
- `tails`: top frequent 1-byte suffixes.
- `checksums`: how many lines match `sum`/`xor` of data-portion (last byte as check).
- `suggestedFrameLen`: most common line byte-length, only if ≥50% of lines share it and ≥`minRepeat`.
`minRepeat` default 2. Use as hints, then confirm with `/decode`.
```bash
curl -sS -H "Authorization: Bearer $SERIALTOOL_TOKEN" \
  "$SERIALTOOL_URL/sessions/s1/infer?dir=rx&minRepeat=3"
```

## Interaction probes (gated by `allowSend`; default OFF -> 403)

### `POST /sessions/:id/send`
Body: `{ mode:'ascii'|'hex', text }`. Returns `{}`.
```bash
curl -sS -X POST -H "Authorization: Bearer $SERIALTOOL_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"mode":"hex","text":"AA 01 02"}' \
  "$SERIALTOOL_URL/sessions/s1/send"
```

### `POST /sessions/:id/exchange`
Send, then capture the first RX line matching `match` within `waitMs`.
Body: `{ send:{mode,text}, waitMs?:2000, match?:{ re?, hex?, dir?:'rx'|'tx' } }`.
`{ sent:true, response:BridgeLine|null, waitedMs }`. `match` defaults to any RX line. `dir` defaults `rx`.
```bash
curl -sS -X POST -H "Authorization: Bearer $SERIALTOOL_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"send":{"mode":"hex","text":"AA 01"},"waitMs":2000,"match":{"hex":"AA 02"}}' \
  "$SERIALTOOL_URL/sessions/s1/exchange"
```

## PlotConfig (grammar) shape
`{ enabled, source:'binary'|'ascii-hex', frameHead, frameTail, checksum:'none'|'sum'|'xor', channels:number, bytesPerChannel:1|2|4, endian:'big'|'little', signed:boolean, maxPoints:number }`. `head`/`tail` are hex strings; `bytes` (per channel) is 1/2/4.
