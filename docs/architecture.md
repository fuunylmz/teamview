# Architecture

TeamView is designed as a native-first real-time media system.

The MVP avoids WebRTC and peer-to-peer delivery. A broadcaster sends low-latency encoded media to a Relay/SFU server over QUIC. The server forwards encoded packets to viewers without decoding, transcoding, compositing, or delaying fast viewers for slow viewers.

The relay owns room and stream lifecycle state. Room creators join automatically, clients leave or unsubscribe during normal shutdown, and disconnect cleanup removes stale subscriptions, publisher-owned streams, stream metrics, keyframe requests, and empty rooms from discovery. Each viewer egress queue has both a bounded datagram capacity and a configurable media-time budget so a slow viewer drops its own queued datagrams instead of adding latency or delaying other viewers.

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
- Microphone source metadata/listing, captured PCM frame validation, a latest-audio capture queue, and a WinMM PCM input path are in place for `--voice-input microphone`.
- The temporary Opus-like voice encoder can emit synthetic samples or embed captured microphone PCM, allowing audio packetization, relay forwarding, reassembly, decode, latest-audio sink playback, and optional WinMM speaker playback to be validated before real Opus is integrated.
- The temporary H.264-like screen encoder embeds a bounded BGRA preview for CPU-backed frames, allowing live screen pixels to travel through packetization, relay forwarding, reassembly, decode, and playback before hardware H.264 is integrated.
- The viewer can render decoded BGRA frames into the latest-frame sink for tests or a native Win32 preview window with `--render-output window`.

The queue defaults to capacity 1. This is intentional: if capture outruns encode/network, the app should drop stale frames and keep realtime behavior instead of accumulating latency.

## First milestones

The first milestones use synthetic media datagrams, pre-encoded sample frames, live CPU screen frames with downsampled BGRA previews wrapped by the synthetic H.264-like encoder, microphone-device discovery, microphone PCM inside the temporary Opus-like voice frame, optional WinMM speaker playback, and synthetic Opus-like voice frames. This proves the server routing model, per-viewer isolation, packetization, decoder-to-playback handoff, audio playback handoff, and low-latency queue policy before interactive Windows Graphics Capture GPU textures, hardware encoding, real Opus, and production window controls are added.

Relay stream metrics include ingress/egress counters, drop counts, current egress queue packet/media depth, subscriber counts, last ingress time, and receive-to-route p50/p95 timing so publisher-side logs can separate server routing and queueing cost from broadcaster and viewer pipeline cost. Clients also issue multi-sample control-plane `TimeSync` requests to estimate relay clock offset before media starts. Forwarded media datagrams carry the sender relay-clock offset estimate, sender encode/send timestamps, and relay receive/send timestamps so viewer logs can report calibrated capture-to-viewer latency, publisher-side media timing, and relay receive-to-send queue delay. Viewer stats split receive-to-reassembly p50/p95 from decode and render timing, which makes jitter/reorder delay visible before native hardware decode lands. Relay feedback can lower publisher bitrate, framerate, and finally screen resolution while keeping the server in forwarding-only mode.

The desktop QUIC control client is cloneable and shares a single atomic request-id sequence across clones. That keeps control responses unambiguous while allowing screen and voice media loops to share one QUIC connection safely.
The broadcaster now uses that shared client to publish screen and voice streams together with `--media-kind both`; the viewer can subscribe to both selected streams and demux screen/voice media packets by stream id on the same connection.
