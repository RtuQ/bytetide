#!/usr/bin/env python3
"""ByteTide bridge monitor — live-edge multi-port follow with anomaly filtering.

Token discipline (why this script is shaped this way):
  - starts from the LIVE EDGE (stats.lastNo): never replays the ring backlog
    (a sinceNo=0 start can dump up to 20k lines into your context);
  - raw lines buffered to <out>/<PORT>.jsonl on disk, never printed;
  - only lines matching --filter reach stdout, each truncated to --print-len;
  - follows the PORT NAME, not the session id: a replug/reflash creates a new
    session id for the same port and this script re-attaches automatically
    (picks the freshest duplicate by lineCount — stale sessions linger);
  - kill it when done (Ctrl-C); re-arm after any reflash/replug is NOT needed
    thanks to name-following, but do restart if the bridge URL changes.

Usage:
  python3 monitor-template.py --filter 'erase warning|async timeout|mismatch' \
      [--only COM3,COM8] [--out /tmp/btmon] [--print-len 200] [--timeout-ms 15000] \
      [--url http://192.168.1.50:8765 --token TOKEN]

URL/token default to $SERIALTOOL_URL / $SERIALTOOL_TOKEN.

If you don't need the raw full-fidelity disk buffer, prefer server-side
follow filtering (implemented):
  follow?sinceNo=N&timeoutMs=T&re=PAT&exclude=NOISE&filterLimit=500
— then only matching lines ever cross the wire and this script collapses to
a plain follow loop. This template keeps local filtering so the jsonl on
disk stays unfiltered (full fidelity for post-hoc /lines?around= pulls).
"""
import argparse
import json
import os
import re
import sys
import threading
import time
import urllib.request

ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
ap.add_argument("--filter", required=True,
                help="regex; ONLY matching lines reach stdout (pre-scan noise first!)")
ap.add_argument("--out", default="/tmp/btmon", help="raw jsonl buffer dir")
ap.add_argument("--only", default="", help="comma list of port names (default: all connected)")
ap.add_argument("--print-len", type=int, default=200)
ap.add_argument("--timeout-ms", type=int, default=15000, help="follow long-poll window")
ap.add_argument("--url", default=os.environ.get("SERIALTOOL_URL", ""))
ap.add_argument("--token", default=os.environ.get("SERIALTOOL_TOKEN", ""))
args = ap.parse_args()

if not args.url or not args.token:
    sys.exit("need --url/--token or $SERIALTOOL_URL/$SERIALTOOL_TOKEN")
H = {"Authorization": "Bearer " + args.token}
ANOM = re.compile(args.filter)
ONLY = {x.strip() for x in args.only.split(",") if x.strip()}
os.makedirs(args.out, exist_ok=True)


def api(path, timeout=25):
    req = urllib.request.Request(args.url.rstrip("/") + path, headers=H)
    return json.load(urllib.request.urlopen(req, timeout=timeout))


def pick(name):
    """Freshest connected session for a port name: highest lineCount wins
    (stale duplicates with the same name linger after replug)."""
    cands = [s for s in api("/sessions")
             if s.get("status") == "connected"
             and s.get("config", {}).get("name") == name]
    return max(cands, key=lambda s: s.get("lineCount", 0)) if cands else None


def follow_port(name):
    f = None
    sid = None
    last = 0
    while True:
        try:
            s = pick(name)
        except Exception:
            s = None
        if s is None:
            time.sleep(3)  # port closed — wait for it to reappear
            continue
        if s["id"] != sid:  # new session (replug/reflash) or first attach
            sid = s["id"]
            try:
                # live edge of the NEW session only; never regress `last` on
                # a known session — that would skip or replay lines
                last = api(f"/sessions/{sid}/stats", timeout=10).get("lastNo", 0)
            except Exception:
                time.sleep(2)
                continue
            if f:
                f.close()
            f = open(os.path.join(args.out, f"{name}.jsonl"), "a", buffering=1)
            print(f"[{name}] following {sid} from no={last}", flush=True)
        try:
            r = api(f"/sessions/{sid}/follow?sinceNo={last}&timeoutMs={args.timeout_ms}")
            for ln in r.get("lines", []):
                no = ln.get("no", last)
                if no > last:
                    last = no
                f.write(json.dumps(ln, ensure_ascii=False) + "\n")
                t = ln.get("text", "")
                if ANOM.search(t):
                    print(f"[{name}] {ln.get('ts', '')} {t[:args.print_len]}", flush=True)
        except Exception:
            time.sleep(2)  # poll failed / session vanished — re-resolve next loop


if __name__ == "__main__":
    try:
        h = api("/health", timeout=10)
        if not h.get("ok"):
            sys.exit(f"bridge not ok: {h}")
    except Exception as e:
        sys.exit(f"bridge unreachable: {e}")
    names = ONLY or sorted({s["config"]["name"]
                            for s in api("/sessions") if s.get("status") == "connected"})
    if not names:
        sys.exit("no connected serial sessions")
    for n in names:
        threading.Thread(target=follow_port, args=(n,), daemon=True).start()
    print(f"monitoring {names} -> {args.out}/  (stdout filter: {args.filter!r})  Ctrl-C to stop",
          flush=True)
    try:
        while True:
            time.sleep(60)
    except KeyboardInterrupt:
        print("bye")
