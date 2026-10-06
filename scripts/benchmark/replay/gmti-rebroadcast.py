#!/usr/bin/env python3
"""Rebroadcast recorded STANAG 4607 (GMTI) packets over TCP, as a live sensor
feed would: listen on a port and stream every packet of the recordings to each
connected client, paced by the dwell times in the data.

Point an OpenTrack source at it with a `tcp_client` transport, 4607 framing
and the `stanag4607` codec plugin (see docs/examples/stanag4607.json).

Usage: gmti-rebroadcast.py <file or directory of .4607 files> [--port 4607]
           [--speed 1.0] [--max-gap 5] [--rate PACKETS_PER_S] [--once]

Pacing: packets go out at the spacing of their dwell times (x --speed), with
gaps in the recording (between files, or the sensor off) cut to --max-gap
seconds; packets without a dwell (mission, job definition...) go straight
after the previous one. --rate sends at a fixed rate instead. Each client
gets the stream from the start, and it repeats until --once.
"""

import argparse
import socket
import struct
import threading
import time
from pathlib import Path

HEADER = 32  # packet header bytes
SEGMENT_HEADER = 5  # segment type (1) + segment size (4)
DWELL = 2
DAY_MS = 86_400_000


def packets(paths):
    """(packet bytes, dwell time in ms after the mission day, or None) in file order."""
    for path in paths:
        data = path.read_bytes()
        i = 0
        while i + 6 <= len(data):
            size = struct.unpack(">I", data[i + 2 : i + 6])[0]
            if size < HEADER or i + size > len(data):
                print(f"{path.name}: bad packet size {size} at byte {i}; rest of file skipped", flush=True)
                break
            yield data[i : i + size], dwell_ms(data[i : i + size])
            i += size


def dwell_ms(packet):
    """The first dwell segment's dwell time: after its 8-byte existence mask,
    revisit index (2), dwell index (2), last-dwell flag (1) and target count (2)."""
    j = HEADER
    while j + SEGMENT_HEADER <= len(packet):
        kind, size = packet[j], struct.unpack(">I", packet[j + 1 : j + 5])[0]
        if size < SEGMENT_HEADER:
            return None
        if kind == DWELL and j + SEGMENT_HEADER + 19 <= len(packet):
            return struct.unpack(">I", packet[j + SEGMENT_HEADER + 15 : j + SEGMENT_HEADER + 19])[0]
        j += size
    return None


def stream(conn, peer, paths, args):
    try:
        while True:
            sent, clock, last = 0, time.monotonic(), None
            for pkt, ms in packets(paths):
                if args.rate:
                    clock += 1 / args.rate
                elif ms is not None:
                    if last is not None:
                        step = (ms - last) / 1000 / args.speed
                        if step < 0 and step > -DAY_MS / 2000:
                            step = 0  # dwells slightly out of order
                        clock += min(abs(step), args.max_gap)
                    last = ms
                wait = clock - time.monotonic()
                if wait > 0:
                    time.sleep(wait)
                conn.sendall(pkt)
                sent += 1
            print(f"{peer}: sent {sent} packets", flush=True)
            if args.once:
                break
    except (BrokenPipeError, ConnectionResetError):
        print(f"{peer}: disconnected", flush=True)
    finally:
        conn.close()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("path")
    ap.add_argument("--port", type=int, default=4607)
    ap.add_argument("--bind", default="127.0.0.1")
    ap.add_argument("--speed", type=float, default=1.0, help="multiple of real time")
    ap.add_argument("--max-gap", type=float, default=5.0, help="longest pause, in seconds")
    ap.add_argument("--rate", type=float, help="fixed packets per second instead of dwell times")
    ap.add_argument("--once", action="store_true", help="send the recordings once per client")
    args = ap.parse_args()
    root = Path(args.path).expanduser()
    paths = sorted(root.rglob("*.4607")) if root.is_dir() else [root]
    if not paths:
        raise SystemExit(f"no .4607 files under {root}")
    total = sum(1 for _ in packets(paths))
    print(f"{len(paths)} files, {total} packets; listening on {args.bind}:{args.port}", flush=True)
    srv = socket.create_server((args.bind, args.port))
    while True:
        conn, addr = srv.accept()
        peer = f"{addr[0]}:{addr[1]}"
        print(f"{peer}: connected", flush=True)
        threading.Thread(target=stream, args=(conn, peer, paths, args), daemon=True).start()


if __name__ == "__main__":
    main()
