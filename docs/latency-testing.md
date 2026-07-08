# Latency Testing

The latency target is below 200 ms glass-to-glass for same-region viewers on good public internet connections.

## Initial success criteria

- Same LAN typical latency below 120 ms once live capture exists.
- Same-region good-network typical latency below 200 ms once live capture exists.
- Synthetic relay tests prove that one slow viewer does not increase fast viewer queue depth or forwarding latency.
- The viewer buffer drops late frames instead of accumulating delay.

## Measurement plan

Early milestones measure synthetic packet forwarding latency. Later milestones add capture, encode, server receive, server send, viewer receive, decode, and render timestamps.

High-speed camera validation should be used to calibrate in-app estimates once live rendering exists.
