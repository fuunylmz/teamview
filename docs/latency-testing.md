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

Stage 3 validates encoded-frame packetization and reassembly before hardware encoding exists.

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

The key invariant is that the capture queue keeps only the newest frame by default. If three frames arrive before encode/network consumes them, the first two are dropped and only the latest frame is returned. On Windows, the live primary-monitor, indexed-monitor, and exact-title visible-window paths can also acquire CPU BGRA pixels; the temporary screen encoder carries a downsampled BGRA preview today, and hardware encoding uses the same frame storage in a later stage.

Covered by unit tests:

- `latest_frame_queue_keeps_only_latest_frame_by_default`
- `latest_frame_queue_capacity_is_never_zero`
- `capture_returns_latest_queued_frame`
- `list_capture_sources_includes_monitors`
- `list_capture_sources_flag_parses_without_relay_options`
- `support_detection_matches_target_os`
- `primary_monitor_size_is_available_on_windows`
- `capture_source_size_uses_primary_monitor_path`
- `capture_source_size_accepts_monitor_index`
- `monitor_capture_source_requires_id`
- `monitor_capture_source_uses_monitor_id`
- `window_capture_source_requires_title`
- `window_capture_source_uses_title_as_id_and_label`

Smoke test:

```bash
cargo run -p desktop-client -- --mode broadcaster --capture-source primary-monitor
```

On Windows, expected output includes `capture_supported=true`.

Capture source listing smoke test:

```bash
cargo run -p desktop-client -- --list-capture-sources
```

Expected output includes at least one `capture-source kind=monitor` line. Visible windows with titles are listed as `capture-source kind=window` lines.

Microphone source listing smoke test:

```bash
cargo run -p desktop-client -- --list-audio-sources
```

Expected output includes `audio-source kind=device` lines on Windows systems with microphone input devices. This validates the local voice-device discovery path before real microphone capture and Opus encoding are wired into the media loop.

Live primary-monitor smoke test:

```bash
cargo run -p relay-server -- --listen 127.0.0.1:4433 --viewer-queue-budget-ms 100
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --screen-input live --media-frames 1 --media-fps 1
```

Expected output includes `screen_input=Live` and the captured `capture_width` / `capture_height`. The live frame currently feeds the synthetic H.264-like encoder with a downsampled BGRA preview so transport, stream config, timestamps, viewer rendering, and relay metrics can be validated before hardware H.264 is added.

Live monitor capture uses a zero-based display index or `primary`:

```bash
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --screen-input live --capture-source monitor --monitor-id 0 --media-frames 1 --media-fps 1
```

The expected output is the same shape as primary-monitor capture, with dimensions matching the selected monitor bounds.

Live window capture uses an exact visible window title:

```bash
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --screen-input live --capture-source window --window-title "Untitled - Notepad" --media-frames 1 --media-fps 1
```

The expected output is the same shape as primary-monitor capture, with `capture_width` and `capture_height` matching the selected window bounds.

## Synthetic QUIC forwarding checks

The current relay/client smoke path validates QUIC datagram media forwarding with synthetic H.264-like frames before hardware encoding and production native decoding land.

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

The desktop client can run a paced synthetic media session over the relay. During startup it sends multiple `TimeSync` samples, logs each sample's relay RTT and `clock_offset_micros`, and uses the lowest-RTT sample for media timing. The broadcaster uses a frame interval derived from `--media-fps`, stamps synthetic captures, copies its relay-clock offset estimate into media packets, stamps encode completion and datagram send time with Unix epoch microseconds, keeps sequence numbers continuous across fragments, records capture/encode/packetize/send timing percentiles, and lingers briefly after finite sends so in-flight datagrams can drain. The relay stamps forwarded datagrams with server receive/send timestamps. The viewer reassembles frames, parses synthetic Annex B H.264-like NAL units into BGRA preview frames, renders them into a latest-frame playback sink or optional native Win32 preview window, estimates capture-to-viewer latency from `sender_capture_time_micros`, computes `calibrated_latency_ms` from sender and viewer relay-clock offsets, logs publisher capture-to-encode/send timing and relay receive-to-send queue delay, tracks packet loss from sequence gaps, records reassembly/decode/render timing percentiles and render FPS, periodically sends `ViewerStats` over the control stream, and sends control-plane keepalives while waiting for delayed media.

Run in separate terminals:

```bash
cargo run -p relay-server -- --listen 127.0.0.1:4433
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --media-run-ms 1000 --media-start-delay-ms 2000 --media-fps 5 --media-frame-bytes 800 --max-datagram-payload 700 --feedback-interval-frames 2
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --room-name stage1 --media-run-ms 1000 --media-fps 5 --max-datagram-payload 700
```

For a visible preview, add `--render-output window` to the viewer command.

If the relay is started with `--access-token`, pass the same `--access-token` to both desktop-client commands.

Expected behavior:

- The broadcaster prints five `media-send` lines at 5 fps for a 1000 ms run, including per-frame `capture_ms`, `encode_ms`, `packetize_ms`, and `send_ms`.
- Both clients print `time-sync-sample` lines and a selected `time-sync` line with the lowest observed relay RTT and `clock_offset_micros` after `Hello`. Use `--time-sync-samples` and `--time-sync-spacing-ms` to tune startup sampling.
- The broadcaster publishes `StreamConfig`, sets target bitrate/framerate, and the viewer polls config before media receive.
- Long `--media-start-delay-ms` and `--media-idle-timeout-ms` windows are kept alive with `Ping`/`Pong` control messages.
- The viewer receives, decodes, and renders five frames split across ten packets with `--max-datagram-payload 700`.
- Each decoded frame prints a `media-render` line with render timestamp, BGRA buffer size, decode time, render time, and render FPS; `--render-output window` also blits the frame into a native preview window.
- The final broadcaster summary includes capture/encode/packetize/send p50 and p95 timing.
- Each received frame prints `latency_ms`, `calibrated_latency_ms`, `sender_encode_ms`, `sender_send_ms`, `server_queue_ms`, and `reassembly_ms`, and the final viewer summary includes latest estimated and calibrated latency plus sender encode/send, server queue, reassembly, decode, and render p50/p95 timing.
- The viewer reassembly buffer drops stale incomplete frames after `--reassembly-window-frames` to avoid accumulating latency.
- The relay enforces each viewer egress queue's media-time budget, dropping over-budget datagrams for that viewer without blocking other viewers.
- The viewer sends periodic `ViewerStats` and receives `PublisherFeedback` responses.
- New subscribers, packet loss, and decoder recovery can register keyframe requests with the relay.
- The broadcaster polls aggregated `PublisherFeedback`; when feedback requests a keyframe, the synthetic encoder marks the next frame as a keyframe.
- The broadcaster polls relay `StreamMetrics` at the end of the run to report server-observed ingress, cumulative queued/dropped egress datagrams, current egress queue packet/media depth, and server route p50/p95 timing.
- When most viewers are degraded by packet loss, dropped frames, excessive jitter/latency, slow reassembly/decode/render p95, or low render FPS, relay feedback lowers the synthetic target bitrate, then framerate, then screen resolution when the earlier targets are already at their floor. The broadcaster shrinks subsequent synthetic frame payloads, rescales screen frames when needed, emits a keyframe after a resolution change, and updates `StreamConfig`.
- The viewer unsubscribes and leaves on normal exit; when the last participant leaves, the relay removes the empty room from subsequent discovery.
- The final viewer summary reports zero loss and drops on a healthy local run.

## Desktop synthetic voice checks

The same desktop client path can publish a synthetic Opus-like voice stream with `--media-kind voice`. The relay validates it as `MediaKind::Voice`, forwards it as audio datagrams, and the viewer reassembles, decodes, and plays frames into a latest-audio playback sink.

Run in separate terminals after starting the relay:

```bash
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --media-kind voice --media-run-ms 1000 --media-start-delay-ms 2000 --media-fps 50 --media-frame-bytes 96 --feedback-interval-frames 10
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --room-name stage1 --media-kind voice --media-run-ms 1000 --media-fps 50
```

Expected behavior:

- The broadcaster publishes an Opus voice stream config and prints `audio-send` lines with capture/encode/packetize/send timing.
- The viewer prints `audio-recv` and `audio-play` lines for each decoded frame, including estimated/calibrated latency, sender encode/send, server queue, reassembly, decode/play timing, and playback FPS.
- The broadcaster polls relay `StreamMetrics`; a healthy single-viewer run reports queued egress datagrams, zero drops, current egress queue depth, and server route timing percentiles.
- The final viewer summary reports `kind=voice`, matching decoded and played frame counts, and zero loss on a healthy local run.

## Measurement plan

Early milestones measure synthetic packet forwarding latency, queue behavior, encoded-frame reassembly behavior, capture queue behavior, live primary-monitor acquisition, synthetic QUIC forwarding behavior, synthetic voice forwarding behavior, synthetic capture-to-viewer latency, multi-sample relay clock offset estimates, TimeSync-derived calibrated capture-to-viewer latency, broadcaster capture/encode/packetize/send timing, publisher stamped capture-to-encode/send timing, server receive-to-route timing, relay receive-to-send queue timing, viewer receive-to-reassembly timing, viewer decode/render timing, and render/playback FPS. Later milestones add hardware encode, continuous calibrated cross-machine clock offset filtering, viewer receive, decode, and render timestamp calibration.

High-speed camera validation should be used to calibrate in-app estimates once live rendering exists.
