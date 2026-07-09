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

The relay defaults each viewer egress queue to a 100 ms media budget. For weak-network tests, adjust it with `--viewer-queue-budget-ms`.

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

For a local desktop-client synthetic media session, start the relay, then start a broadcaster and viewer in separate terminals:

```bash
cargo run -p relay-server -- --listen 127.0.0.1:4433
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --media-run-ms 1000 --media-start-delay-ms 2000 --media-fps 5 --media-frame-bytes 800 --max-datagram-payload 700 --feedback-interval-frames 2
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --room-name stage1 --media-run-ms 1000 --media-fps 5 --max-datagram-payload 700
```

Add `--render-output window` to the viewer to display decoded BGRA frames in a native Win32 preview window instead of only recording the latest-frame sink.

Expected output includes startup `time-sync-sample` lines plus a selected `time-sync` line with the lowest observed relay RTT and `clock_offset_micros`, the broadcaster publishing `StreamConfig`, setting target bitrate/framerate, printing `media-send` lines with capture/encode/packetize/send timing, the viewer polling config before media, publisher feedback polling, relay `stream-metrics`, received frames with `latency_ms` and `calibrated_latency_ms`, `media-render` lines, periodic `viewer-stats` responses, optional `publisher-adapt` lines with bitrate/FPS/resolution targets under degradation, final `unsubscribe-stream` / `leave-room` responses, and a final viewer summary similar to:

```text
stream-metrics stream_id=1 ingress_packets=10 ingress_bytes=... egress_queue_packets=0 egress_queue_media_ms=0 server_route_ms_p50=0 server_route_ms_p95=1
media-summary role=broadcaster frames=5 packets=10 fps=5 run_ms=1000 capture_ms_p50=0 capture_ms_p95=1 encode_ms_p50=0 encode_ms_p95=1 packetize_ms_p50=0 packetize_ms_p95=0 send_ms_p50=0 send_ms_p95=0
media-summary role=viewer frames=5 decoded=5 rendered=5 packets=10 lost=0 dropped=0 latency_ms=1 calibrated_latency_ms=1 sender_encode_ms_p50=0 sender_encode_ms_p95=1 sender_send_ms_p50=0 sender_send_ms_p95=1 server_queue_ms_p50=0 server_queue_ms_p95=1 reassembly_ms_p50=0 reassembly_ms_p95=1 decode_ms_p50=0 decode_ms_p95=1 render_ms_p50=0 render_ms_p95=1 render_fps=5
```

For a local synthetic voice session, use `--media-kind voice` and a 50 fps packet cadence:

```bash
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --media-kind voice --media-run-ms 1000 --media-start-delay-ms 2000 --media-fps 50 --media-frame-bytes 96 --feedback-interval-frames 10
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --room-name stage1 --media-kind voice --media-run-ms 1000 --media-fps 50
```

Expected voice output includes `audio-send` with capture/encode/packetize/send timing, `audio-recv` with `latency_ms`, `calibrated_latency_ms`, sender encode/send, server queue, and reassembly timing, `audio-play` with decode/play timing, relay `stream-metrics`, and final broadcaster/viewer summaries similar to `media-summary role=broadcaster kind=voice frames=50 ... encode_ms_p95=0 ...` and `media-summary role=viewer kind=voice frames=50 decoded=50 played=50 ... calibrated_latency_ms=1 ... sender_send_ms_p95=0 ... server_queue_ms_p95=0 ... play_fps=50`.

To use a real Windows microphone as the voice input, list devices first and pass `--voice-input microphone`. `--microphone-id` is optional; without it the default WinMM capture device is used.

```bash
cargo run -p desktop-client -- --list-audio-sources
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --media-kind voice --voice-input microphone --microphone-id 0 --media-run-ms 1000 --media-start-delay-ms 2000 --media-fps 50 --feedback-interval-frames 10
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --room-name stage1 --media-kind voice --media-run-ms 1000 --media-fps 50
```

The microphone path captures 16-bit PCM with WinMM and carries those samples inside the temporary Opus-like test payload so the relay, packetization, reassembly, latency metrics, and audio playback handoff can be exercised before real Opus encoding is added.

## Current stage

Stage 4 plus synthetic QUIC media forwarding: the desktop client has a Windows capture foundation with support detection, capture source metadata, capture-source listing, microphone source listing and WinMM PCM capture, frame metadata, a latest-frame queue that keeps only the newest frame to avoid latency buildup, and live CPU BGRA acquisition paths for the primary monitor, indexed monitors, and exact-title visible windows behind `--screen-input live`. The temporary H.264-like screen encoder now embeds a downsampled BGRA preview for CPU-backed live frames, so the viewer can decode and render actual captured screen pixels through the relay path before hardware H.264 lands. The temporary Opus-like voice encoder can send synthetic audio or embed captured microphone PCM samples so the viewer can play the same samples through the relay path before real Opus lands. The viewer can either record decoded frames in the latest-frame sink or show them in a native Win32 preview window with `--render-output window`. The relay can also forward validated synthetic media datagrams from a publisher to subscribed viewers through independent bounded viewer egress queues with a configurable media-time budget, optionally require a shared access token, list rooms and streams for viewer discovery, store and serve stream config, clean up empty rooms and publisher-owned streams when clients leave or disconnect, stamp forwarded media with relay receive/send timestamps, expose stream ingress/egress metrics plus server route timing percentiles, aggregate viewer stats into publisher feedback, and the client/load-test paths can estimate relay clock offset with multi-sample `TimeSync`, stamp publisher clock offset into media packets, packetize, pace, send, receive, reassemble with stale-frame drops, parse synthetic Annex B H.264-like frames into BGRA preview frames, send and receive synthetic Opus-like voice frames, estimate capture-to-viewer latency from media timestamps, report calibrated capture-to-viewer latency from sender/viewer relay-clock offsets, stamp and report publisher capture-to-encode/send timing, measure broadcaster capture/encode/packetize/send timing, measure relay receive-to-send queue timing, measure viewer reassembly/decode/render timing and render/playback FPS, report viewer stats, poll publisher feedback and stream metrics, request synthetic keyframes for new subscribers, packet loss, or decoder recovery, adapt synthetic bitrate/FPS/resolution targets when viewers are degraded, update `StreamConfig` after resolution changes, send unsubscribe/leave messages on normal exit, and keep QUIC control connections alive while waiting for delayed media.

Interactive Windows Graphics Capture source picking/GPU texture capture, real Opus, hardware encode, native decode, production window controls, and production-grade adaptive media feedback are later stages.
