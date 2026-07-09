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

The first milestones validate the QUIC transport, control plane, Relay/SFU state model, slow-viewer isolation, encoded-frame packetization, and low-latency capture queueing before implementing hardware encoding.

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
cargo run -p desktop-client -- --mode broadcaster --capture-source primary-monitor
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

For the Stage 4 capture foundation smoke test:

```bash
cargo run -p desktop-client -- --mode broadcaster --capture-source primary-monitor
```

Expected output includes `capture_supported=true` on Windows.

For the synthetic QUIC media forwarding smoke test:

```bash
cargo run -p load-test -- --mode quic-sample-forward --viewers 2 --packets 2 --max-payload 700
```

Expected output shows fragmented synthetic H.264-like frames sent through the relay over QUIC datagrams and reassembled by every viewer:

```text
quic-sample-forward frames=2 fragments=14 reassembled=4 delivered=28 dropped=0
```

For a local desktop-client synthetic media session, start the relay, then start a broadcaster and viewer in separate terminals:

```bash
cargo run -p relay-server -- --listen 127.0.0.1:4433
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --media-run-ms 1000 --media-start-delay-ms 2000 --media-fps 5 --media-frame-bytes 800 --max-datagram-payload 700 --feedback-interval-frames 2
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --room-id 1 --media-run-ms 1000 --media-fps 5 --max-datagram-payload 700
```

Expected output includes the broadcaster publishing `StreamConfig`, the viewer polling it before media, publisher feedback polling, received frames, periodic `viewer-stats` responses, and a final viewer summary similar to:

```text
media-summary role=viewer frames=5 decoded=5 packets=10 lost=0 dropped=0
```

## Current stage

Stage 4 plus synthetic QUIC media forwarding: the desktop client has a Windows capture foundation with support detection, capture source metadata, frame metadata, and a latest-frame queue that keeps only the newest frame to avoid latency buildup. The relay can also forward validated synthetic media datagrams from a publisher to subscribed viewers through independent bounded viewer egress queues, store and serve stream config, aggregate viewer stats into publisher feedback, and the client/load-test paths can packetize, pace, send, receive, reassemble with stale-frame drops, parse synthetic Annex B H.264-like frames, report viewer stats, poll publisher feedback, and request a synthetic keyframe when needed.

Actual Windows Graphics Capture frame acquisition, hardware encode, native decode, real rendering, and adaptive media feedback are later stages.
