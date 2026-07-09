# Architecture

TeamView is designed as a native-first real-time media system.

The MVP avoids WebRTC and peer-to-peer delivery. A broadcaster sends low-latency encoded media to a Relay/SFU server over QUIC. The server forwards encoded packets to viewers without decoding, transcoding, compositing, or delaying fast viewers for slow viewers.

The relay owns room and stream lifecycle state. Room creators join automatically, clients leave or unsubscribe during normal shutdown, and disconnect cleanup removes stale subscriptions, publisher-owned streams, stream metrics, keyframe requests, and empty rooms from discovery. Each subscribed viewer stream has a bounded egress datagram queue and configurable media-time budget, so a slow viewer or a backed-up stream drops its own queued datagrams instead of adding latency, delaying other viewers, or making screen and voice streams contend for the same latency budget.

Remote input uses the reliable control plane. Subscribed screen viewers can enqueue bounded pointer/key/text events for a screen stream, and only that stream's publisher can poll them. Broadcasters default to logging those events, and can opt in to `--remote-input-output native` to inject them into the local Windows desktop with `SendInput`. This keeps input routing tied to the same room, stream, and authorization model as media before native viewer-side input capture is wired.

## Primary pipeline

```text
Windows capture
  -> low-latency hardware H.264 encode
  -> packetization into MTU-safe QUIC datagrams
  -> relay ingress
  -> per-viewer per-stream bounded egress queues
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
- Codec backend listing is available with `--list-codec-backends`. The screen broadcaster now has a `--video-encoder synthetic|media-foundation` selection point, and the viewer has a `--video-decoder synthetic|media-foundation` selection point; the Media Foundation options probe Windows for hardware H.264 encoder/decoder MFT availability and fail early with that detail until frame submission/output is wired.
- The temporary Opus-like voice encoder can emit synthetic samples or embed captured microphone PCM with independent voice cadence and payload sizing, allowing audio packetization, relay forwarding, reassembly, decode, latest-audio sink playback, and optional WinMM speaker playback to be validated before real Opus is integrated.
- Room-scoped voice state is tracked by the relay. `--muted` suppresses client voice sending, `--push-to-talk` / `--ptt-active` model an idle or pressed talk key, and `--deafened` suppresses relay voice delivery plus local playback for that viewer. Room participants can list participant display names, voice state, and published/subscribed stream counts with `--list-participants`.
- Scripted remote input can be sent by screen viewers with `--remote-input-script`; the relay queues those events for the screen publisher, and the broadcaster polls them during the screen publishing loop. `--remote-input-output log` is the default safe sink; `--remote-input-output native` maps pointer/key/text events to Win32 `SendInput`.
- The temporary H.264-like screen encoder embeds a bounded BGRA preview for CPU-backed frames, allowing live screen pixels to travel through packetization, relay forwarding, reassembly, decode, and playback before hardware H.264 is integrated.
- The viewer can render decoded BGRA frames into the latest-frame sink for tests or a native Win32 preview window with `--render-output window`.

The queue defaults to capacity 1. This is intentional: if capture outruns encode/network, the app should drop stale frames and keep realtime behavior instead of accumulating latency.

## First milestones

The first milestones use synthetic media datagrams, pre-encoded sample frames, live CPU screen frames with downsampled BGRA previews wrapped by the synthetic H.264-like encoder, Media Foundation H.264 encoder/decoder backend probing, microphone-device discovery, microphone PCM inside the temporary Opus-like voice frame, optional WinMM speaker playback, synthetic Opus-like voice frames, scripted remote-input events, and optional broadcaster-side native input injection. This proves the server routing model, per-viewer isolation, packetization, decoder-to-playback handoff, audio playback handoff, low-latency queue policy, codec-backend selection points, and input authorization/queueing before interactive Windows Graphics Capture GPU textures, hardware encoder frame submission, native decoder frame output, real Opus, native viewer input capture, and production window controls are added.

Relay stream metrics include ingress/egress counters, drop counts, current egress queue packet/media depth, subscriber counts, last ingress time, and receive-to-route p50/p95 timing so publisher-side logs can separate server routing and queueing cost from broadcaster and viewer pipeline cost. Clients issue multi-sample control-plane `TimeSync` requests to estimate relay clock offset before media starts, then refresh that offset during media runs. Forwarded media datagrams carry the sender relay-clock offset estimate, sender encode/send timestamps, and relay receive/send timestamps so viewer logs can report calibrated capture-to-viewer latency, publisher-side media timing, and relay receive-to-send queue delay. Viewer stats split receive-to-reassembly p50/p95 from decode and render timing, which makes jitter/reorder delay visible before native hardware decode lands. Relay feedback can lower publisher bitrate, framerate, and finally screen resolution while keeping the server in forwarding-only mode.

The desktop QUIC control client is cloneable and shares a single atomic request-id sequence across clones. That keeps control responses unambiguous while allowing screen and voice media loops to share one QUIC connection safely.
The broadcaster now uses that shared client to publish screen and voice streams together with `--media-kind both`; the viewer can subscribe to both selected streams and demux screen/voice media packets by stream id on the same connection. Screen and voice streams keep separate FPS and payload sizing knobs so channel voice does not inherit screen-share packet cadence or synthetic payload size. Publishers can explicitly `UnpublishStream` a single screen or voice stream, allowing screen-share stop flows to clear subscriptions and per-stream relay state without leaving the channel.
The desktop client also exposes a channel UI snapshot contract: `--export-ui-state` / `--print-ui-state` convert relay room, participant, screen stream, voice stream, and local voice/screen controls into the same camelCase JSON model consumed by `apps/desktop-ui`. `--serve-ui` embeds the same static console, serves `/state.json` from live relay discovery, accepts `POST /api/screen-share` so broadcaster-mode screen controls publish or unpublish the relay screen stream and run the screen media sender while sharing is active, and accepts `POST /api/voice-state` so mute, deafen, and push-to-talk controls update relay voice state before the UI refreshes. This keeps the frontend aligned with the native client state while a production desktop shell is still being wired.
