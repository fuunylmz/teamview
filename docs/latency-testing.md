# Latency Testing

The latency target is below 200 ms glass-to-glass for same-region viewers on good public internet connections.

## Initial success criteria

- Same LAN typical latency below 120 ms once live capture exists.
- Same-region good-network typical latency below 200 ms once live capture exists.
- Synthetic relay tests prove that one slow viewer does not increase fast viewer queue depth or forwarding latency.
- The viewer buffer drops late frames instead of accumulating delay.
- The capture pipeline drops stale frames instead of letting a capture queue grow.

## Stage 2 synthetic fanout checks

Stage 2 validates slow-viewer isolation before real network media I/O exists.

The deterministic simulation uses one synthetic publisher, N viewers, and fixed-duration synthetic media packets. Fast viewers drain one packet before each new packet arrives. The optional slow viewer does not drain, so its bounded queue fills and drops packets.

Run:

```bash
cargo run -p load-test -- --publishers 1 --viewers 4 --packets 10 --include-slow-viewer
```

Expected behavior:

- Fast viewers continue receiving packets.
- The slow viewer drops packets after its queue budget is exhausted.
- Total drops equal `slow_viewer_dropped` in this deterministic scenario.
- The publisher and fast viewers are not blocked by the slow viewer.

Example output:

```text
synthetic-fanout publishers=1 viewers=4 packets=10 delivered=31 dropped=9 slow_viewer_dropped=9
```

## Stage 3 encoded sample forwarding checks

Stage 3 validates encoded-frame packetization and reassembly before live capture or hardware encoding exists.

Run:

```bash
cargo run -p load-test -- --mode sample-forward --viewers 2 --packets 3 --max-payload 700
```

Expected behavior:

- Each H.264-like sample frame is split into multiple media packet fragments.
- Fanout delivers each fragment to viewers through the same queue model used by Stage 2.
- The first viewer reassembles every frame.
- Reassembled bytes exactly match the original encoded frame bytes.
- Missing or incomplete fragment sets are rejected by protocol tests.

Example output:

```text
sample-forward frames=3 fragments=18 reassembled=3 delivered=36 dropped=0
```

## Stage 4 capture queue checks

Stage 4 validates capture-side latency policy without requiring interactive screen selection.

The key invariant is that the capture queue keeps only the newest frame by default. If three frames arrive before encode/network consumes them, the first two are dropped and only the latest frame is returned.

Covered by unit tests:

- `latest_frame_queue_keeps_only_latest_frame_by_default`
- `latest_frame_queue_capacity_is_never_zero`
- `capture_returns_latest_queued_frame`
- `support_detection_matches_target_os`

Smoke test:

```bash
cargo run -p desktop-client -- --mode broadcaster --capture-source primary-monitor
```

On Windows, expected output includes `capture_supported=true`.

## Synthetic QUIC forwarding checks

The current relay/client smoke path validates QUIC datagram media forwarding with synthetic H.264-like frames before live capture, hardware encoding, native decoding, or rendering exists.

Run:

```bash
cargo run -p load-test -- --mode quic-sample-forward --viewers 2 --packets 2 --max-payload 700
```

Expected behavior:

- A publisher creates a room, publishes a screen stream, and sends fragmented synthetic frames as QUIC datagrams.
- Each viewer joins, subscribes, receives every forwarded fragment, and reassembles each frame byte-for-byte.
- The relay accepts publisher media into independent bounded viewer egress queues and drops only for viewers whose queue is full.
- Relay forwarding rejects media from non-publishers and packet types/codecs that do not match the published stream.

Example output:

```text
quic-sample-forward frames=2 fragments=14 reassembled=4 delivered=28 dropped=0
```

## Desktop synthetic session checks

The desktop client can run a paced synthetic media session over the relay. The broadcaster uses a frame interval derived from `--media-fps`, keeps sequence numbers continuous across fragments, and lingers briefly after finite sends so in-flight datagrams can drain. The viewer reassembles frames, parses synthetic Annex B H.264-like NAL units, tracks packet loss from sequence gaps, and periodically sends `ViewerStats` over the control stream.

Run in separate terminals:

```bash
cargo run -p relay-server -- --listen 127.0.0.1:4433
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --media-run-ms 1000 --media-start-delay-ms 2000 --media-fps 5 --media-frame-bytes 800 --max-datagram-payload 700 --feedback-interval-frames 2
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --room-id 1 --media-run-ms 1000 --media-fps 5 --max-datagram-payload 700
```

Expected behavior:

- The broadcaster prints five `media-send` lines at 5 fps for a 1000 ms run.
- The broadcaster publishes `StreamConfig`, and the viewer polls it before media receive.
- The viewer receives and decodes five frames split across ten packets with `--max-datagram-payload 700`.
- The viewer reassembly buffer drops stale incomplete frames after `--reassembly-window-frames` to avoid accumulating latency.
- The viewer sends periodic `ViewerStats` and receives `PublisherFeedback` responses.
- The broadcaster polls aggregated `PublisherFeedback`; when feedback requests a keyframe, the synthetic encoder marks the next frame as a keyframe.
- The final viewer summary reports zero loss and drops on a healthy local run.

## Measurement plan

Early milestones measure synthetic packet forwarding latency, queue behavior, encoded-frame reassembly behavior, capture queue behavior, and synthetic QUIC forwarding behavior. Later milestones add real capture, encode, server receive, server send, viewer receive, decode, and render timestamps.

High-speed camera validation should be used to calibrate in-app estimates once live rendering exists.
