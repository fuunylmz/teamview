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

The relay defaults each subscribed viewer stream egress queue to a 100 ms media budget. For weak-network tests, adjust it with `--viewer-queue-budget-ms`.
The relay also drops ingress media datagrams larger than `--max-datagram-payload`, which defaults to the protocol datagram target. Keep this aligned with the desktop client's `--max-datagram-payload` when testing custom MTU budgets.
Room capacity is bounded with `--max-rooms`, `--max-participants-per-room`, and `--max-streams-per-room`.

To require a shared access token for control actions and media datagrams:

```bash
cargo run -p relay-server -- --listen 127.0.0.1:4433 --access-token dev-secret
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --access-token dev-secret
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

To inspect available capture targets before broadcasting:

```bash
cargo run -p desktop-client -- --list-capture-sources
```

Expected output includes `capture-source kind=monitor ...` lines for displays and `capture-source kind=window ...` lines for visible titled windows.

To inspect available microphone inputs before voice work:

```bash
cargo run -p desktop-client -- --list-audio-sources
```

Expected output includes `audio-source kind=device ...` lines on Windows systems with microphone input devices.

To inspect codec backend readiness before switching away from the synthetic media path:

```bash
cargo run -p desktop-client -- --list-codec-backends
```

Expected output includes synthetic H.264 encoder/decoder lines and Media Foundation H.264 hardware encoder/decoder probe lines. Use `--video-encoder media-foundation` or `--video-decoder media-foundation` to explicitly request those backends; the client now fails early with a capability/detail message if no hardware H.264 MFT is available or if the selected backend has not yet wired frame submission/output.

To inspect channel participants and their TeamSpeak-style voice state after a broadcaster has created a channel:

```bash
cargo run -p desktop-client -- --list-rooms
cargo run -p desktop-client -- --list-streams --channel-name stage1
cargo run -p desktop-client -- --list-participants --channel-name stage1 --display-name Alice
```

Expected output includes `room ...`, `stream ...`, and `participant ... muted=... deafened=... push_to_talk=... speaking=... published_streams=... subscribed_streams=...` lines.

`--channel-name` and `--channel-id` are the preferred CLI names for channel media sessions. The older `--room-name` and `--room-id` flags remain supported as protocol-compatible aliases.

The first frontend workspace is a static channel console at `apps/desktop-ui/index.html`. It models the desktop channel UI for screen sharing, voice controls, participant state, and channel switching while the native Rust media path remains the source of truth for relay/client behavior. The desktop client can serve that UI locally and expose live relay discovery at `/state.json`.

```bash
cargo run -p desktop-client -- --relay 127.0.0.1:4433 --channel-name stage1 --media-kind both --serve-ui --ui-listen 127.0.0.1:7788
```

Open `http://127.0.0.1:7788/` to load the channel console. In broadcaster mode, the screen share control calls the client-local API to publish or unpublish the relay screen stream and runs the screen media sender while sharing is active. In `--media-kind voice` or `--media-kind both`, the voice controls also publish the channel voice stream and run the voice media sender while the local voice state is speaking; muting, deafening, or releasing inactive push-to-talk stops the sender and unpublishes that voice stream. For bridge/debug tooling, `--export-ui-state apps/desktop-ui/state.json` writes the same channel snapshot to disk and `--print-ui-state` prints it to stdout.

To exercise live primary-monitor acquisition through the desktop broadcaster, start a relay and run one live screen frame:

```bash
cargo run -p relay-server -- --listen 127.0.0.1:4433 --viewer-queue-budget-ms 100
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --screen-input live --media-frames 1 --media-fps 1
```

Expected output includes `screen_input=Live` with the captured `capture_width` and `capture_height`. The current live path captures CPU BGRA pixels from the primary monitor and carries a downsampled BGRA preview through the synthetic H.264-like test encoder until hardware encoding lands.

To capture a specific monitor by zero-based display index, use `--capture-source monitor --monitor-id`:

```bash
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --screen-input live --capture-source monitor --monitor-id 0 --media-frames 1 --media-fps 1
```

The special monitor id `primary` selects the primary display.

To capture a specific visible window instead of the full primary monitor, pass an exact window title:

```bash
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --screen-input live --capture-source window --window-title "Untitled - Notepad" --media-frames 1 --media-fps 1
```

The window path uses the same CPU BGRA preview pipeline and reports that window's captured dimensions.

For the synthetic QUIC media forwarding smoke test:

```bash
cargo run -p load-test -- --mode quic-sample-forward --viewers 2 --packets 2 --max-payload 700
```

Expected output shows fragmented synthetic H.264-like frames sent through the relay over QUIC datagrams and reassembled by every viewer:

```text
quic-sample-forward frames=2 fragments=14 reassembled=4 delivered=28 dropped=0
```

For an in-process relay/client channel smoke that publishes both screen and voice streams through one channel:

```bash
cargo run -p load-test -- --mode quic-channel-media --viewers 2 --packets 2 --max-payload 700
```

Expected output includes separate screen and voice reassembly counts, for example `quic-channel-media screen_frames=2 voice_frames=2 ... screen_reassembled=4 voice_reassembled=4 ... dropped=0`.

For a local desktop-client synthetic media session, start the relay, then start a broadcaster and viewer in separate terminals:

```bash
cargo run -p relay-server -- --listen 127.0.0.1:4433
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --channel-name stage1 --media-run-ms 1000 --media-start-delay-ms 2000 --media-fps 5 --media-frame-bytes 800 --max-datagram-payload 700 --feedback-interval-frames 2
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --channel-name stage1 --media-run-ms 1000 --media-fps 5 --max-datagram-payload 700
```

Add `--render-output window` to the viewer to display decoded BGRA frames in a native Win32 preview window instead of only recording the latest-frame sink.

For a scripted remote-input smoke, add `--remote-input-script pointer-tap` to a screen viewer. The viewer sends bounded control-plane input events to the relay after subscribing, and the broadcaster polls and logs `remote-input` events while publishing the screen stream. Broadcasters default to `--remote-input-output log`; use `--remote-input-output native` only when you intentionally want the broadcaster to inject those events into the local Windows desktop with `SendInput`.

Expected output includes startup `time-sync-sample` lines plus a selected `time-sync` line with the lowest observed relay RTT and `clock_offset_micros`, the broadcaster publishing `StreamConfig`, setting target bitrate/framerate, printing `media-send` lines with capture/encode/packetize/send timing, the viewer polling config before media, publisher feedback polling, relay `stream-metrics`, received frames with `latency_ms` and `calibrated_latency_ms`, `media-render` lines, periodic `viewer-stats` responses, optional `publisher-adapt` lines with bitrate/FPS/resolution targets under degradation, final `unsubscribe-stream` / `leave-room` responses, and a final viewer summary similar to:

```text
stream-metrics stream_id=1 ingress_packets=10 ingress_bytes=... egress_queue_packets=0 egress_queue_media_ms=0 server_route_ms_p50=0 server_route_ms_p95=1
media-summary role=broadcaster frames=5 packets=10 fps=5 run_ms=1000 capture_ms_p50=0 capture_ms_p95=1 encode_ms_p50=0 encode_ms_p95=1 packetize_ms_p50=0 packetize_ms_p95=0 send_ms_p50=0 send_ms_p95=0
media-summary role=viewer frames=5 decoded=5 rendered=5 packets=10 lost=0 dropped=0 latency_ms=1 calibrated_latency_ms=1 sender_encode_ms_p50=0 sender_encode_ms_p95=1 sender_send_ms_p50=0 sender_send_ms_p95=1 server_queue_ms_p50=0 server_queue_ms_p95=1 reassembly_ms_p50=0 reassembly_ms_p95=1 decode_ms_p50=0 decode_ms_p95=1 render_ms_p50=0 render_ms_p95=1 render_fps=5
```

During media runs the desktop client refreshes relay clock offset with `TimeSync` every 5000 ms by default and logs `time-sync-refresh` samples. Use `--time-sync-refresh-ms 0` to disable refreshes for deterministic timing tests.

The viewer bounds incomplete-frame jitter by media time. `--jitter-buffer-max-ms` defaults to 150 ms; when incomplete frames exceed that budget, the viewer drops the oldest incomplete frames instead of accumulating latency.

For a local synthetic voice session, use `--media-kind voice`. Voice defaults to a 50 fps packet cadence and 96-byte temporary Opus-like payloads, which gives 20 ms audio frames; use `--voice-fps` and `--voice-frame-bytes` to tune it separately from screen frame rate and payload size.

```bash
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --channel-name stage1 --media-kind voice --media-run-ms 1000 --media-start-delay-ms 2000 --voice-fps 50 --voice-frame-bytes 96 --feedback-interval-frames 10
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --channel-name stage1 --media-kind voice --media-run-ms 1000 --voice-fps 50
```

Add `--audio-output speaker` to the viewer to queue decoded PCM to the default Windows speaker through WinMM instead of only recording the latest-audio sink.

Use `--muted` on a voice broadcaster to publish the room/stream state without sending voice frames. Use `--push-to-talk` to require an active press before sending voice; add `--ptt-active` to mark the current run as speaking. Use `--deafened` on a viewer to update relay voice state, suppress voice delivery, and avoid local audio playback.

Expected voice output includes `audio-send` with capture/encode/packetize/send timing, `audio-recv` with `latency_ms`, `calibrated_latency_ms`, sender encode/send, server queue, and reassembly timing, `audio-play` with decode/play timing, relay `stream-metrics`, and final broadcaster/viewer summaries similar to `media-summary role=broadcaster kind=voice frames=50 ... encode_ms_p95=0 ...` and `media-summary role=viewer kind=voice frames=50 decoded=50 played=50 ... calibrated_latency_ms=1 ... sender_send_ms_p95=0 ... server_queue_ms_p95=0 ... play_fps=50`.

To publish channel screen sharing and channel voice from one broadcaster connection, use `--channel-name` with `--media-kind both`. The screen stream uses `--stream-id`, `--media-fps`, and `--media-frame-bytes`; the voice stream defaults to the next id, can be set with `--voice-stream-id`, and uses independent `--voice-fps` and `--voice-frame-bytes` settings. A viewer can also use `--media-kind both` to subscribe to both selected streams on one connection and demux packets by stream id.

```bash
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --channel-name stage1 --media-kind both --stream-id 1 --voice-stream-id 2 --media-run-ms 1000 --media-start-delay-ms 2000 --media-fps 30 --voice-fps 50 --media-frame-bytes 800 --voice-frame-bytes 96 --feedback-interval-frames 10
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --channel-name stage1 --media-kind both --stream-id 1 --voice-stream-id 2 --media-run-ms 1000 --media-fps 30 --voice-fps 50
```

To use a real Windows microphone as the voice input, list devices first and pass `--voice-input microphone`. `--microphone-id` is optional; without it the default WinMM capture device is used.

```bash
cargo run -p desktop-client -- --list-audio-sources
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --channel-name stage1 --media-kind voice --voice-input microphone --microphone-id 0 --media-run-ms 1000 --media-start-delay-ms 2000 --voice-fps 50 --voice-frame-bytes 96 --feedback-interval-frames 10
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --channel-name stage1 --media-kind voice --media-run-ms 1000 --voice-fps 50
```

The microphone path captures 16-bit PCM with WinMM and carries those samples inside the temporary Opus-like test payload so the relay, packetization, reassembly, latency metrics, latest-audio sink, and optional speaker playback handoff can be exercised before real Opus encoding is added.

## Current stage

Stage 4 plus synthetic QUIC media forwarding: the desktop client has a Windows capture foundation with support detection, capture source metadata, capture-source listing, microphone source listing and WinMM PCM capture, codec-backend listing with Media Foundation H.264 hardware encoder/decoder probing, frame metadata, a latest-frame queue that keeps only the newest frame to avoid latency buildup, and live CPU BGRA acquisition paths for the primary monitor, indexed monitors, and exact-title visible windows behind `--screen-input live`. The temporary H.264-like screen encoder now embeds a downsampled BGRA preview for CPU-backed live frames, so the viewer can decode and render actual captured screen pixels through the relay path before hardware H.264 lands. The broadcaster can choose `--video-encoder synthetic` or explicitly request `--video-encoder media-foundation`; the viewer can choose `--video-decoder synthetic` or explicitly request `--video-decoder media-foundation`. The Media Foundation paths currently perform capability validation and are ready for frame-submission/frame-output implementation. The temporary Opus-like voice encoder can send synthetic audio or embed captured microphone PCM samples so the viewer can play the same samples through the relay path before real Opus lands. A broadcaster can publish screen and voice streams together from one QUIC connection with `--media-kind both`, and a viewer can subscribe to both selected streams in one process and demux screen/voice packets by stream id. Voice playback can use the latest-audio sink or queue PCM to the default Windows speaker with `--audio-output speaker`; `--muted`, `--push-to-talk` / `--ptt-active`, and `--deafened` now update relay voice state and locally suppress voice send/playback. The served channel console can now drive the same screen-share sender and channel voice sender from its controls, including unpublishing each stream independently when the control state turns off. The viewer can either record decoded frames in the latest-frame sink or show them in a native Win32 preview window with `--render-output window`. Screen viewers can also send scripted remote input with `--remote-input-script`, and screen broadcasters poll relay-queued input events while publishing; `--remote-input-output native` can inject those events into the local Windows desktop through `SendInput`. The relay can also forward validated synthetic media datagrams from a publisher to subscribed viewers through bounded per-viewer per-stream egress queues with configurable media-time budgets, reject voice datagrams from muted or inactive push-to-talk publishers, suppress voice delivery to deafened viewers, optionally require a shared access token, list rooms, streams, and room participants for viewer discovery, expose participant display names, voice state, and published/subscribed stream counts, queue bounded remote-input events from subscribed screen viewers for the screen publisher, store and serve stream config, clean up empty rooms and publisher-owned streams when clients leave or disconnect, stamp forwarded media with relay receive/send timestamps, expose stream ingress/egress metrics plus server route timing percentiles, aggregate viewer stats into publisher feedback, and the client/load-test paths can estimate relay clock offset with multi-sample startup `TimeSync`, refresh that offset during media runs, stamp publisher clock offset into media packets, packetize, pace, send, receive, reassemble with stale-frame and jitter-budget drops, parse synthetic Annex B H.264-like frames into BGRA preview frames, send and receive synthetic Opus-like voice frames, estimate capture-to-viewer latency from media timestamps, report calibrated capture-to-viewer latency from sender/viewer relay-clock offsets, stamp and report publisher capture-to-encode/send timing, measure broadcaster capture/encode/packetize/send timing, measure relay receive-to-send queue timing, measure viewer reassembly/decode/render timing and render/playback FPS, report viewer stats, poll publisher feedback, remote input, and stream metrics, request synthetic keyframes for new subscribers, packet loss, or decoder recovery, adapt synthetic bitrate/FPS/resolution targets when viewers are degraded, update `StreamConfig` after resolution changes, send unsubscribe/leave messages on normal exit, and keep QUIC control connections alive while waiting for delayed media.

Interactive Windows Graphics Capture source picking/GPU texture capture, Media Foundation H.264 frame submission/output, real Opus, native viewer-side input capture, production window controls, and production-grade adaptive media feedback are later stages.
