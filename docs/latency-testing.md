# Latency Testing

The latency target is below 200 ms glass-to-glass for same-region viewers on good public internet connections.

## Initial success criteria

- Same LAN typical latency below 120 ms once live capture exists.
- Same-region good-network typical latency below 200 ms once live capture exists.
- Synthetic relay tests prove that one slow viewer does not increase fast viewer queue depth or forwarding latency.
- The viewer buffer drops late frames instead of accumulating delay.

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

## Measurement plan

Early milestones measure synthetic packet forwarding latency and queue behavior. Later milestones add capture, encode, server receive, server send, viewer receive, decode, and render timestamps.

High-speed camera validation should be used to calibrate in-app estimates once live rendering exists.
