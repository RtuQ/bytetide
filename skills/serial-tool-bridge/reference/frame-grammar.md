# Frame grammar & data model

Authoritative spec for the byte-stream → value[] decode performed by `/decode` and `/value-hist`. This is an independent Rust port of the frontend `usePlotParser.parseFrames`; the grammar here is the single source of truth for the bridge.

## BridgeLine model

```
BridgeLine = {
  no:          u64,        // monotonic per-session, starts at 1, never reused
  ts:          string,     // wall-clock "HH:MM:SS.mmm" (local) for human display
  dir:         'rx' | 'tx',
  text:        string,     // decoded text (lossy UTF-8; invalid bytes -> U+FFFD)
  bytes:       number[] | null,  // raw bytes; see "bytes rules" below
  epochMillis: i64,       // UNIX epoch ms (UTC) for filtering/histogram/timing
  match?: { offset, length, field:'text'|'bytes' }  // present only when filters matched
}
```

### bytes rules
- `bytes` is the **raw** byte array of the line, as received on the serial port (before any text decoding).
- For lines whose text is valid UTF-8, the bridge MAY omit `bytes` (it is `null`); reconstruct bytes from `text.encode()` if you need them.
- For lines containing **invalid UTF-8** (e.g. raw binary frames), `bytes` is always populated — this is the only faithful representation.
- Only the **recent ring** (default 20000 lines, see `ringCap` in `/health`) retains `bytes`. Lines beyond the ring are gone; there is no full-history byte endpoint. If you need bytes, work within the ring window.
- TX lines carry the bytes that were **sent**; RX lines carry bytes **received**. `dir` distinguishes them.

## Frame layout

A frame is a contiguous byte run with this layout:

```
[ HEADER ] [ DATA: channels × bytesPerChannel ] [ CHECKSUM? ] [ TAIL? ]
```

- **HEADER** (`head`): fixed hex byte sequence, e.g. `AA55`. Empty = no header (decode starts at every offset).
- **DATA**: `channels` values, each `bytesPerChannel` bytes (1, 2, or 4), laid out consecutively.
- **CHECKSUM** (`checksum`): one trailing byte, present iff `checksum ≠ 'none'`.
  - `sum`: low byte of the sum of all bytes **after** the header (data + checksum-excluded) → compared with `& 0xff`.
  - `xor`: byte-wise XOR of the same range.
- **TAIL** (`tail`): fixed hex byte sequence after the checksum (if any). Empty = none.

Total frame length = `head.len() + channels×bytesPerChannel + (1 if checksum≠none else 0) + tail.len()`.

## Scan algorithm

Given a byte stream (concatenated RX line bytes within the requested `no` range):

1. Walk offsets. At each offset, try to match `head` (if set). If `head` is empty, every offset is a candidate start.
2. From a candidate start, read `head.len() + channels×bytesPerChannel + checksumByte + tail.len()` bytes. If the run runs past available bytes, stop (incomplete frame).
3. Verify `tail` (if set) and `checksum` (if set). If either fails, this is **not** a frame: advance by **1 byte** (not by frame length) and retry. Record the failure position in `lastError`.
4. On success: decode each channel's value, emit a `DecodeFrame`, advance by the full frame length, and continue.

This sliding-window-with-1-byte-advance-on-failure matches the frontend parser exactly: a missed sync can resync within a few bytes rather than skipping a whole frame.

## parseValue (channel bytes → number)

For a channel's `bytesPerChannel` raw bytes, in the configured `endian`:

1. Assemble an unsigned integer `u`:
   - `big`: `u = b[0]<<8 | b[1]` etc. (most-significant first).
   - `little`: `u = b[n-1]<<8 | ... | b[0]`.
   - Computed via **u128 accumulation** (`u = u*256 + byte`) to avoid truncation for 4-byte values that overflow i32 range.
2. If `signed` is true: signed interpretation via two's complement.
   - Threshold `half = 256^bytesPerChannel / 2`.
   - If `u >= half`, the signed value is `u - 2*half` (i.e. `u - 256^bytesPerChannel`).
   - Else the signed value equals `u`.
3. The result is the numeric `values[i]` for that channel.

Examples (`bytesPerChannel=2`, bytes `[0xFF, 0x7C]`):
- big, unsigned: `0xFF7C = 65404`.
- big, signed: `65404 - 65536 = -132`.
- little, unsigned: `0x7CFF = 31999`.
- little, signed: `31999 - 65536 = -33537`.

## Checksum

Both kinds operate on the byte range **between** the header and the checksum byte itself (i.e. the DATA bytes; the tail, if any, is **not** included in the checksum input):

- `sum`: `expected = (sum of DATA bytes) & 0xFF`. Compare to the trailing checksum byte.
- `xor`: `expected = DATA bytes[0] ^ DATA bytes[1] ^ ... ^ DATA bytes[n-1]`. Compare to the trailing checksum byte.

If `checksum = 'none'`, no trailing byte is consumed and no verification occurs.

## ascii-hex source mode

When `source = 'ascii-hex'`, the "byte stream" is **not** the raw line bytes. Instead, the parser extracts hex byte pairs from the line's **text**:

- Scan the text for runs matching `[0-9a-fA-F]{2}` (case-insensitive, non-overlapping, left to right).
- Each matched pair becomes one byte in the decode input (`0x3F`, etc.).
- Non-hex characters (spaces, `0x` prefixes, punctuation, line noise) are skipped.
- If a line yields zero hex pairs, it contributes nothing to the stream.

This is for protocols that **print** their frames as hex text (e.g. `AA 55 01 02 00 58`). Use `/infer` to detect whether a stream looks ascii-hex: if `heads`/`tails` show meaningful hex prefixes on text, switch `source=ascii-hex`.

In `binary` source (default), the decode input is the raw `bytes` of each line in `no` order, concatenated.

## Recommended workflow

1. `/stats` — see `firstNo`/`lastNo`/`size` and RX byte count. If `size` is near `ringCap`, you only have the last ~20000 lines; sample with `?last=` or `?around=`.
2. `/infer?dir=rx` — candidate `heads`/`tails`/`checksums`/`suggestedFrameLen`. Use as guesses.
3. `/plot-config` — what the user already configured; reuse it if it looks right.
4. `/decode?head=...&checksum=...&channels=...&bytes=...` — confirm with a small `?limit=` first; check `frameCount`, `lastError`, and whether `valid` is mostly true.
5. If `valid` is mostly false and `lastError` recurs at the same offset, the grammar is wrong — retry with `/infer` suggestions or ask the user.
6. `/value-hist?...&channel=0` — value distribution per channel to spot periodic/saturated values.
7. `/histogram` + `/timing` — arrival pattern (burst vs steady, inter-frame gaps).
8. `/lines?around=<badNo>&span=5` — pull context around a flagged frame to inspect raw `bytes`/`text` directly.

## Edge cases

- **Mixed valid/invalid UTF-8 lines**: invalid-UTF-8 RX lines keep `bytes`; their `text` contains `U+FFFD`. For `binary` decode, rely on `bytes`. For `ascii-hex`, rely on `text` (hex survives UTF-8 lossy decode if it's ASCII hex).
- **Empty ring**: `/lines` returns `lines:[]`, `total:0`, `firstNo:0`, `lastNo:0`. `/decode` returns `frameCount:0`. `/stats` returns zero counters.
- **TX lines in decode**: `source=binary` includes TX bytes by default unless you filter `?dir=rx`. Most protocols want `dir=rx` for decode. `/infer` defaults to `dir=rx`.
- **`head` empty + `tail` empty + `checksum none`**: every offset is a "frame" of length `channels×bytesPerChannel` — useful only as a raw value sampler.
- **Truncated final frame**: if the ring ends mid-frame, the partial run is dropped (no partial frame emitted); `scanned` reflects bytes examined.
