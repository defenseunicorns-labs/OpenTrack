"""An example gRPC producer for OpenTrack's protobuf sources, on grpcio.

    python producer.py serve --port 50061            # OpenTrack subscribes (grpc_client)
    python producer.py push --target 127.0.0.1:50051 --token $TOKEN   # pushes (grpc_server)
    python producer.py push --target localhost:50051 --ca ca.pem --cert client.pem --key client.key   # over mutual TLS

It streams N vessels on straight courses, a batch a second. The stubs are
generated from acme_tracks.proto at start (grpcio-tools), as a producer would
from its own schema.
"""

import argparse
import importlib
import math
import sys
import tempfile
import time
from concurrent import futures
from pathlib import Path

import grpc
from grpc_tools import protoc

HERE = Path(__file__).resolve().parent


def stubs():
    out = tempfile.mkdtemp(prefix="acme-stubs-")
    wkt = Path(protoc.__file__).parent / "_proto"
    rc = protoc.main(["protoc", f"-I{HERE}", f"-I{wkt}", f"--python_out={out}", f"--grpc_python_out={out}",
                      str(HERE / "acme_tracks.proto")])
    if rc != 0:
        raise SystemExit("protoc failed")
    sys.path.insert(0, out)
    return importlib.import_module("acme_tracks_pb2"), importlib.import_module("acme_tracks_pb2_grpc")


pb, rpc = stubs()


def batch(n, t0):
    now = time.time()
    b = pb.TrackBatch()
    for k in range(n):
        course, speed = (37 * k) % 360, 4 + k % 10
        d = speed * (now - t0)
        lat = 50.70 + 0.01 * (k % 20) + math.degrees(d * math.cos(math.radians(course)) / 6371008.8)
        lon = -1.40 + 0.02 * (k // 20) + math.degrees(
            d * math.sin(math.radians(course)) / (6371008.8 * math.cos(math.radians(lat))))
        t = b.tracks.add(track_id=f"ACME-{k:04d}", course=course, speed=speed, mmsi=235100000 + k,
                         name=f"ACME {k}")
        t.position.lat, t.position.lon = lat, lon
        t.time.FromSeconds(int(now))
    return b


class Feed(rpc.TrackFeedServicer):
    def __init__(self, n):
        self.n = n

    def Stream(self, request, context):
        print(f"subscriber: {context.peer()} area={request.area!r}", flush=True)
        t0 = time.time()
        while context.is_active():
            yield batch(self.n, t0)
            time.sleep(1)

    def Push(self, request_iterator, context):
        context.abort(grpc.StatusCode.UNIMPLEMENTED, "this producer only streams")


def serve(a):
    server = grpc.server(futures.ThreadPoolExecutor(max_workers=8))
    rpc.add_TrackFeedServicer_to_server(Feed(a.tracks), server)
    server.add_insecure_port(f"127.0.0.1:{a.port}")
    server.start()
    print(f"serving TrackFeed on 127.0.0.1:{a.port} ({a.tracks} tracks)", flush=True)
    server.wait_for_termination()


def push(a):
    t0 = time.time()
    metadata = [("authorization", f"Bearer {a.token}")] if a.token else []

    def batches():
        for i in range(a.seconds):
            yield batch(a.tracks, t0)
            time.sleep(1)

    if a.ca:
        read = lambda p: Path(p).read_bytes() if p else None
        creds = grpc.ssl_channel_credentials(read(a.ca), read(a.key), read(a.cert))
        channel = grpc.secure_channel(a.target, creds)
    else:
        channel = grpc.insecure_channel(a.target)
    with channel as ch:
        try:
            ack = rpc.TrackFeedStub(ch).Push(batches(), metadata=metadata, timeout=a.seconds + 30)
            print(f"pushed {a.seconds} batches; OpenTrack acknowledged ({type(ack).__name__})", flush=True)
        except grpc.RpcError as e:
            print(f"push failed: {e.code().name}: {e.details()}", flush=True)
            raise SystemExit(1)


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("serve")
    s.add_argument("--port", type=int, default=50061)
    s.add_argument("--tracks", type=int, default=50)
    p = sub.add_parser("push")
    p.add_argument("--target", default="127.0.0.1:50051")
    p.add_argument("--token")
    p.add_argument("--tracks", type=int, default=50)
    p.add_argument("--seconds", type=int, default=30)
    p.add_argument("--ca", help="PEM CA that signed OpenTrack's certificate: TLS")
    p.add_argument("--cert", help="PEM client certificate, for mutual TLS")
    p.add_argument("--key", help="its PEM key")
    a = ap.parse_args()
    serve(a) if a.cmd == "serve" else push(a)
