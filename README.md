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

The first milestone validates the transport and Relay/SFU behavior before implementing real screen capture or hardware encoding.

## Workspace

```text
crates/protocol       Shared packet, control, codec, and stats types
crates/relay-server   QUIC Relay/SFU server
crates/desktop-client Native broadcaster/viewer client foundation
crates/load-test      Synthetic publisher/viewer load testing tool
```

## Development commands

```bash
cargo fmt
cargo test
cargo build
cargo run -p relay-server -- --help
cargo run -p desktop-client -- --help
cargo run -p load-test -- --help
```

## Current stage

Stage 0: Git repository, Rust workspace, protocol primitives, and runnable binary skeletons.
