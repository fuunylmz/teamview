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

The first milestones validate the QUIC transport, control plane, Relay/SFU state model, and slow-viewer isolation before implementing real screen capture or hardware encoding.

## Workspace

```text
crates/protocol       Shared packet, control, codec, and stats types
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

## Current stage

Stage 2: synthetic media fanout with per-viewer bounded queues. The relay can model delivery and drops per viewer, and tests prove one slow viewer does not block fast viewers.

Real QUIC media datagram I/O and live capture/encode are later stages.
