# Protobuf and gRPC sources

OpenTrack reads protobuf messages defined by the producer's own `.proto` files. You upload the files with the source, and OpenTrack compiles them when it runs, so you don't need `protoc`, generated code or a rebuild. The messages can arrive over gRPC in either direction, or over any other transport.

## The protobuf codec

A source's codec is `protobuf`:

```json
"codec": {
  "type": "protobuf",
  "files": { "acme_tracks.proto": "syntax = \"proto3\"; ..." },
  "message": "acme.tracks.v1.TrackBatch",
  "records": "tracks"
}
```

- **`files`** holds the producer's `.proto` files, name to contents.
  - They may import each other by those names.
  - The `google/protobuf/*.proto` well-known types are built in.
  - The limit is 64 files and 2 MB.
- **`message`** is the full name of the message each frame holds.
- **`records`** is the repeated field whose elements are the records. Without it, the message itself is one record. `context` copies message-level fields into every record under `_frame`, as the JSON codec does.

**How to add the files:** in the UI, go to Sources → Add source (or a source's settings) and choose the Protobuf codec. Add the `.proto` files one at a time. They are compiled as you add them, and an error names the file and line (`tracks.proto:12: expected ';'`). The message and records pickers then list what the files define. From the API, `POST /api/v1/protobuf/describe {"files": {...}}` returns the methods, messages and fields.

**What the mapping sees:** a message becomes a JSON tree, following the proto3 JSON mapping with four differences that make mapping easier:

| | In OpenTrack's records | Standard proto3 JSON |
|---|---|---|
| Field names | as written in the `.proto` (`track_id`) | lowerCamelCase (`trackId`) |
| 64-bit integers | numbers | strings |
| Fields at their default (0, false, "") | included, so a latitude of 0 is 0 | left out |
| `optional` fields that are not set | left out | left out |

Enums are their value names (`KIND_VESSEL`). A `google.protobuf.Timestamp` is an RFC 3339 string, so the mapping's `time` transform reads it. Wrappers are their value. Map it like any JSON feed: `"position.latitude": "position.lat"`.

**It isn't only for gRPC.** Any transport can carry protobuf:
- TCP streams with varint length framing (`"framing": {"type": "length_prefix", "width": "varint"}`);
- one message per UDP datagram, MQTT message or WebSocket message;
- recorded files.

## OpenTrack subscribes: `grpc_client`

OpenTrack calls the producer's method and reads the messages it streams back.

```json
"transport": {
  "type": "grpc_client",
  "url": "https://feed.acme.example:443",
  "method": "acme.tracks.v1.TrackFeed/Stream",
  "request": { "area": "solent" },
  "metadata": { "authorization": "Bearer ${env:ACME_TOKEN}" },
  "tls": { "ca_file": "/certs/acme-ca.pem", "cert_file": "/certs/ot.pem", "key_file": "/certs/ot.key" }
}
```

- **The method must answer with the codec's message.** Picking a method in the UI sets the message for you. Streaming methods are the usual case. A unary method works too, and is called again each time it answers, which amounts to polling with backoff.
- **`request`** is the request message as JSON. Leave it out for an empty request.
- **`metadata`** is sent with the call. Names are lowercase. `${env:NAME}` keeps a secret out of the spec.
- **When the producer ends the call or the connection drops,** OpenTrack calls again. It backs off from 2 s up to 60 s, with jitter so that many sources don't return at the same instant. The source's status shows each failure and its reason, including the producer's own gRPC status and message.
- **`keepalive_secs`** (default 20) sends HTTP/2 pings, so a quiet stream through a NAT or firewall isn't cut silently.
- **`max_message_kib`** (default 4096) is the largest message accepted, gzip-compressed or not.
- **TLS:** `https://` uses TLS with the system's CAs, or with `tls.ca_file`. A client certificate gives mutual TLS. `http://` is plain HTTP/2.

## Producers push: `grpc_server`

OpenTrack listens, and producers call methods of their own `.proto`. Every message they send is a frame.

```json
"transport": {
  "type": "grpc_server",
  "bind": "0.0.0.0:50051",
  "methods": ["acme.tracks.v1.TrackFeed/Push"],
  "token": "${env:ACME_PUSH_TOKEN}",
  "tls": { "cert_file": "/certs/ot.pem", "key_file": "/certs/ot.key", "client_ca_file": "/certs/producers-ca.pem" }
}
```

- **`methods`** are the methods producers may call; each must send the codec's message.
  - Unary and client-streaming calls both work, and a producer can keep one stream open for as long as it likes.
  - Leave `methods` out to accept every method whose request is the codec's message.
  - OpenTrack answers each call with the method's response message left at its defaults.
- **`token`**, if set, must arrive as `authorization: Bearer <token>`. It is compared in constant time.
- **`tls`**: a certificate and key give TLS. `client_ca_file` makes it mutual: only producers holding a certificate that CA signed get in. Each record carries the producer's certificate subject as `_frame.peer_subject`, and its address as the frame's origin.
- A `grpc_server` source needs a `token` or mutual TLS (or both). Without either it is refused unless it carries `"unauthenticated": "accepted"`, a risk acceptance the decision log records ([hardening checklist](security/hardening.md#encrypt-every-link)).
- **`max_connections`** (default 64) is how many producers may be connected at once; more are refused.
- **`max_message_kib`** (default 4096) is the largest message accepted.

**What a producer is told.** A producer learns at once why its call failed, rather than having messages dropped silently:

| gRPC status | When |
|---|---|
| OK | every message was accepted |
| UNAUTHENTICATED | the bearer token is missing or wrong |
| UNIMPLEMENTED | the method isn't accepted here, or the compression isn't gzip |
| INVALID_ARGUMENT | a message isn't a valid instance of the method's request type, with which message and why |
| RESOURCE_EXHAUSTED | a message is over the size limit (a gzip bomb is caught while it decompresses) |
| INTERNAL | not a gRPC call, or the call ended inside a message |

Refused calls and failed TLS handshakes also count in the source's status: errors, and the last one with the producer's address or certificate subject.

**Backpressure, not loss.** When the pipeline falls behind, OpenTrack stops reading the call. HTTP/2 flow control then slows the producer until it catches up. Nothing is dropped.

## Validation

Saving a source checks that:
- the `.proto` files compile;
- the message exists;
- a `grpc_client`'s method exists, answers with the message, and its request encodes;
- a `grpc_server`'s methods exist and send the message.

So a broken source is refused when it is saved, not when data arrives.

## Tried and measured

The example in [`examples/grpc/`](examples/grpc/) has a `.proto`, a producer on Google's `grpcio`, and a source spec for each direction. With it, OpenTrack was checked against `grpcio` 1.84:

| Test | Result |
|---|---|
| Subscribing | 4,800 records; OpenTrack reconnected by itself when the producer was restarted |
| Pushing | acknowledged; a wrong token was refused with UNAUTHENTICATED |
| Mutual TLS | a client with a signed certificate got in; one without was refused, and the source's status said why |
| Load | 16,000 records a second pushed for a minute: all 960,000 arrived with no errors, and the engine kept up |
