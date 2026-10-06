"""Serve a plugin over a socket: OpenTrack's external plugin protocol.

JSON lines. Each request is {"id", "method", "params"}; each reply
{"id", "result"} or {"id", "error"}. Every decoder, tracker or scorer
OpenTrack opens is one connection, which starts with `open`
({"kind", "options"}); the instance lives as long as the connection.

    describe                               -> manifest
    open      kind, options                -> null
    decode    frame (base64), received_at_ms -> records
    hints                                  -> hints or null
    push      plot, received_at_ms         -> null
    run       now_ms, force                -> tracks
    score     report, candidates           -> scores

Every connection first authenticates both ends with the secret OpenTrack and
the plugin share (docs/plugins.md, "Authentication"):

    hello     protocol, challenge (Nc)     -> proof, challenge (Np)
    verify    proof                        -> null

The plugin's proof is HMAC-SHA256(secret, b"opentrack-plugin-v1 plugin\\0" + Nc
+ Np), OpenTrack's the same with "opentrack"; Nc and Np are 32 random bytes,
base64. Nothing else is answered until OpenTrack's proof checks. The secret
comes from OT_PLUGIN_SECRET or a file (`serve --secret-file`), never the
command line. Without one the plugin answers only over a unix socket in
OpenTrack's data directory, where OpenTrack skips the handshake.
"""

from __future__ import annotations

import base64
import hashlib
import hmac
import json
import os
import socket
import socketserver
import sys
import traceback

from opentrack_plugin import Plugin, PluginError, log


PROTOCOL = "opentrack-plugin-v1"
NONCE_LEN = 32
MIN_SECRET_LEN = 32


def proof(secret: str, role: str, opentrack_nonce: bytes, plugin_nonce: bytes) -> bytes:
    """HMAC-SHA256 over b"opentrack-plugin-v1 <role>\\0" + Nc + Np (role: plugin or opentrack)."""
    msg = f"{PROTOCOL} {role}".encode() + b"\0" + opentrack_nonce + plugin_nonce
    return hmac.new(secret.encode(), msg, hashlib.sha256).digest()


class _Handshake:
    """One connection's side of the handshake."""

    def __init__(self, secret: str | None):
        self.secret = secret
        self.challenges: tuple[bytes, bytes] | None = None
        # Without a secret there is nothing to prove: OpenTrack reaches this
        # plugin only over a unix socket in its data directory.
        self.done = secret is None

    def hello(self, p: dict):
        if self.secret is None:
            raise PluginError("this plugin has no secret: start it with OT_PLUGIN_SECRET or --secret-file")
        theirs = base64.b64decode(p.get("challenge") or "")
        if p.get("protocol") != PROTOCOL or len(theirs) != NONCE_LEN:
            raise PluginError(f"expected {PROTOCOL} with a {NONCE_LEN}-byte challenge")
        ours = os.urandom(NONCE_LEN)
        self.challenges = (theirs, ours)
        return {
            "proof": base64.b64encode(proof(self.secret, "plugin", theirs, ours)).decode(),
            "challenge": base64.b64encode(ours).decode(),
        }

    def verify(self, p: dict):
        if self.secret is None or self.challenges is None:
            raise PluginError("verify before hello")
        theirs, ours = self.challenges
        self.challenges = None
        expected = proof(self.secret, "opentrack", theirs, ours)
        given = base64.b64decode(p.get("proof") or "")
        if not hmac.compare_digest(expected, given):
            log("error", "OpenTrack's proof is wrong: refused (does it have this plugin's secret?)")
            raise _Refused("OpenTrack's proof is wrong")
        self.done = True


class _Refused(PluginError):
    """The other end failed the handshake: answer, then hang up."""


def _handler(plugin: Plugin, secret: str | None):
    class Session(socketserver.StreamRequestHandler):
        def handle(self):
            instance, kind = None, None
            auth = _Handshake(secret)
            for line in self.rfile:
                if not line.strip():
                    continue
                req_id = None
                try:
                    req = json.loads(line)
                    req_id = req.get("id")
                    method = req.get("method")
                    p = req.get("params") or {}
                    if method == "hello":
                        result = auth.hello(p)
                    elif method == "verify":
                        result = auth.verify(p)
                    elif not auth.done:
                        raise _Refused(f"{method} before the handshake")
                    elif method == "describe":
                        result = plugin.describe()
                    elif method == "open":
                        kind = p.get("kind")
                        instance = plugin.open(kind, p.get("options") or {})
                        result = None
                    elif instance is None:
                        raise PluginError(f"{method} before open")
                    elif method == "decode" and kind == "codec":
                        result = instance.decode(base64.b64decode(p["frame"]), int(p["received_at_ms"]))
                    elif method == "hints" and kind == "codec":
                        result = instance.hints()
                    elif method == "push" and kind == "tracker":
                        instance.push(p["plot"], int(p["received_at_ms"]))
                        result = None
                    elif method == "run" and kind == "tracker":
                        result = instance.run(int(p["now_ms"]), bool(p.get("force")))
                    elif method == "score" and kind == "scorer":
                        result = instance.score(p["report"], p["candidates"])
                    else:
                        raise PluginError(f"unknown method {method!r} for a {kind}")
                    reply = {"id": req_id, "result": result}
                except _Refused as e:
                    self.wfile.write((json.dumps({"id": req_id, "error": str(e)}) + "\n").encode())
                    self.wfile.flush()
                    return
                except PluginError as e:
                    reply = {"id": req_id, "error": str(e)}
                except Exception as e:  # noqa: BLE001 - reported to OpenTrack, and logged here
                    traceback.print_exc(file=sys.stderr)
                    reply = {"id": req_id, "error": f"{type(e).__name__}: {e}"}
                self.wfile.write((json.dumps(reply, default=str) + "\n").encode())
                self.wfile.flush()

    return Session


class _Tcp(socketserver.ThreadingTCPServer):
    daemon_threads = True
    allow_reuse_address = True


class _Unix(socketserver.ThreadingUnixStreamServer):
    daemon_threads = True


def load_secret(secret_file: str | None = None) -> str | None:
    """The shared secret: from `secret_file`, else OT_PLUGIN_SECRET; None if neither."""
    if secret_file:
        with open(secret_file, encoding="utf-8") as f:
            secret = f.read().strip()
    else:
        secret = os.environ.get("OT_PLUGIN_SECRET", "").strip()
    if not secret:
        return None
    if len(secret) < MIN_SECRET_LEN:
        raise SystemExit(f"the plugin secret must be at least {MIN_SECRET_LEN} characters")
    return secret


def serve(plugin: Plugin, address: str, secret: str | None = None) -> None:
    """Serve until interrupted: one thread per connection. Every connection
    must prove it holds `secret` (and is shown this plugin does) before
    anything else; with no secret, only OpenTrack's unix-socket case works."""
    if secret is None:
        log("warning", "no secret (OT_PLUGIN_SECRET or --secret-file): OpenTrack connects only over a unix socket in its data directory")
    handler = _handler(plugin, secret)
    if address.startswith("unix:"):
        path = address[len("unix:"):]
        if os.path.exists(path):
            os.unlink(path)
        server = _Unix(path, handler)
    else:
        host, _, port = address.removeprefix("tcp://").rpartition(":")
        server = _Tcp((host or "127.0.0.1", int(port)), handler)
        server.socket.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    m = plugin.describe()
    log("info", f"{m['name']} {m['version']} ({', '.join(m['kinds'])}) serving on {address}")
    with server:
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass
