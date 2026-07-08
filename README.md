# TeamView

TeamView is an experimental native-first real-time voice and screen sharing system inspired by TeamSpeak-style voice channels and Discord-style screen sharing.

The core media goal is public-internet multi-viewer screen sharing with a server-distributed Relay/SFU architecture, no WebRTC, no server-side transcoding in the MVP, and a same-region good-network target below 200 ms glass-to-glass latency.

## Architecture direction

```text
Capture
  -> low-latency encode
  -> QUIC datagrams to relay
  -> Relay/SFU forwarding
  -> low-buffer decode and playback
```

The first milestones validate the QUIC transport, control plane, Relay/SFU state model, slow-viewer isolation, and encoded-frame packetization before implementing real screen capture or hardware encoding.

## Workspace

```text
crates/protocol       Shared packet, frame, control, codec, and stats types
crates/relay-server   QUIC Relay/SFU server and reusable routing primitives
crates/desktop-client Native broadcaster/viewer client foundation
crates/load-test      Synthetic publisher/viewer load testing tool
```

## Development commands

```bash
cargo fmt
cargo test
cargo build
cargo clippy --all-targets -- -D warnings
cargo run -p relay-server -- --listen 127.0.0.1:4433
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433
cargo run -p load-test -- --publishers 1 --viewers 10 --packets 120 --include-slow-viewer
```

For a smoke test that starts the relay and exits after a few seconds:

```bash
timeout 3s cargo run -p relay-server -- --listen 127.0.0.1:0 || true
```

For the Stage 2 slow-viewer isolation simulation:

```bash
cargo run -p load-test -- --publishers 1 --viewers 4 --packets 10 --include-slow-viewer
```

Expected output includes drops only for the slow viewer, for example:

```text
synthetic-fanout publishers=1 viewers=4 packets=10 delivered=31 dropped=9 slow_viewer_dropped=9
```

For the Stage 3 pre-encoded H.264-like sample forwarding simulation:

```bash
cargo run -p load-test -- --mode sample-forward --viewers 2 --packets 3 --max-payload 700
```

Expected output shows frames split into multiple fragments, relayed to viewers, and reassembled byte-for-byte:

```text
sample-forward frames=3 fragments=18 reassembled=3 delivered=36 dropped=0
```

## Current stage

Stage 3: pre-encoded H.264-like sample frames can be packetized into MTU-safe media packets, forwarded through the existing fanout model, and reassembled by a viewer with byte-for-byte validation.

Real QUIC media datagram I/O, live capture, hardware encode, decode, and rendering are later stages.
