# Architecture

TeamView is designed as a native-first real-time media system.

The MVP avoids WebRTC and peer-to-peer delivery. A broadcaster sends low-latency encoded media to a Relay/SFU server over QUIC. The server forwards encoded packets to viewers without decoding, transcoding, compositing, or delaying fast viewers for slow viewers.

The relay owns room and stream lifecycle state. Room creators join automatically, clients leave or unsubscribe during normal shutdown, and disconnect cleanup removes stale subscriptions, publisher-owned streams, stream metrics, keyframe requests, and empty rooms from discovery.

## Primary pipeline

```text
Windows capture
  -> low-latency hardware H.264 encode
  -> packetization into MTU-safe QUIC datagrams
  -> relay ingress
  -> per-viewer bounded egress queues
  -> QUIC datagram downlink
  -> tiny jitter/reorder buffer
  -> native decode
  -> playback
```

## Capture foundation

Stage 4 introduces the desktop capture abstraction without depending on an interactive screen picker or GPU frame API in tests.

The capture layer provides:

- `CaptureSource` for primary monitor, monitor id, or window id/title.
- Capture source listing for displays and visible titled windows before starting a relay session.
- `CaptureFrame` metadata with frame id, dimensions, capture timestamp, pixel format, and storage kind, including validated CPU BGRA buffers.
- `LatestFrameQueue`, a bounded queue that drops older frames and keeps the newest frame.
- `WindowsGraphicsCapture` support detection, a test-frame path for non-interactive verification, and live CPU BGRA capture via GDI for primary-monitor, indexed-monitor, and exact-title visible-window sources in the `--screen-input live` broadcaster path.
- The temporary H.264-like screen encoder embeds a bounded BGRA preview for CPU-backed frames, allowing live screen pixels to travel through packetization, relay forwarding, reassembly, decode, and playback before hardware H.264 is integrated.
- The viewer can render decoded BGRA frames into the latest-frame sink for tests or a native Win32 preview window with `--render-output window`.

The queue defaults to capacity 1. This is intentional: if capture outruns encode/network, the app should drop stale frames and keep realtime behavior instead of accumulating latency.

## First milestones

The first milestones use synthetic media datagrams, pre-encoded sample frames, live CPU screen frames with downsampled BGRA previews wrapped by the synthetic H.264-like encoder, and synthetic Opus-like voice frames. This proves the server routing model, per-viewer isolation, packetization, decoder-to-playback handoff, audio playback handoff, and low-latency queue policy before interactive Windows Graphics Capture GPU textures, microphone capture, hardware encoding, real Opus, and production window controls are added.

Relay stream metrics include ingress/egress counters, drop counts, subscriber counts, last ingress time, and receive-to-route p50/p95 timing so publisher-side logs can separate server routing cost from broadcaster and viewer pipeline cost. Viewer stats also split receive-to-reassembly p50/p95 from decode and render timing, which makes jitter/reorder delay visible before native hardware decode lands.
