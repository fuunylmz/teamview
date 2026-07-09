# Architecture

TeamView is designed as a native-first real-time media system.

The MVP avoids WebRTC and peer-to-peer delivery. A broadcaster sends low-latency encoded media to a Relay/SFU server over QUIC. The server forwards encoded packets to viewers without decoding, transcoding, compositing, or delaying fast viewers for slow viewers.

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

Stage 4 introduces the desktop capture abstraction without yet depending on an interactive screen picker or GPU frame API in tests.

The capture layer provides:

- `CaptureSource` for primary monitor, monitor id, or window id/title.
- `CaptureFrame` metadata with frame id, dimensions, capture timestamp, pixel format, and storage kind.
- `LatestFrameQueue`, a bounded queue that drops older frames and keeps the newest frame.
- `WindowsGraphicsCapture` support detection and a test-frame path for non-interactive verification.

The queue defaults to capacity 1. This is intentional: if capture outruns encode/network, the app should drop stale frames and keep realtime behavior instead of accumulating latency.

## First milestones

The first milestones use synthetic media datagrams, pre-encoded sample frames, and synthetic Opus-like voice frames before real capture. This proves the server routing model, per-viewer isolation, packetization, decoder-to-playback handoff, audio playback handoff, and low-latency queue policy before hardware capture, microphone capture, hardware encoding, real Opus, and native window rendering are added.
