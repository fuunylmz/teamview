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

Expected output includes `audio-source kind=device` lines on Windows systems with microphone input devices. This validates the local voice-device discovery path before choosing a live microphone input for the voice media loop.
The broadcaster can also use one of those devices with `--voice-input microphone`; the current path captures WinMM PCM and embeds it in the temporary Opus-like payload until real Opus is integrated.

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
- The relay accepts publisher media into independent bounded viewer/stream egress queues and drops only for viewers whose relevant stream queue is full.
- Relay forwarding rejects media from non-publishers and packet types/codecs that do not match the published stream.

Example output:

```text
quic-sample-forward frames=2 fragments=14 reassembled=4 delivered=28 dropped=0
```

## Desktop synthetic session checks

The desktop client can run a paced synthetic media session over the relay. During startup it sends multiple `TimeSync` samples, logs each sample's relay RTT and `clock_offset_micros`, and uses the lowest-RTT sample for initial media timing. During media runs it refreshes the relay-clock offset every `--time-sync-refresh-ms` milliseconds, defaulting to 5000 and allowing `0` to disable refreshes. The broadcaster uses a frame interval derived from `--media-fps`, stamps synthetic captures, copies its current relay-clock offset estimate into media packets, stamps encode completion and datagram send time with Unix epoch microseconds, keeps sequence numbers continuous across fragments, records capture/encode/packetize/send timing percentiles, and lingers briefly after finite sends so in-flight datagrams can drain. The relay stamps forwarded datagrams with server receive/send timestamps. The viewer reassembles frames with a local incomplete-frame media-time cap from `--jitter-buffer-max-ms`, defaulting to 150, parses synthetic Annex B H.264-like NAL units into BGRA preview frames, renders them into a latest-frame playback sink or optional native Win32 preview window, estimates capture-to-viewer latency from `sender_capture_time_micros`, computes `calibrated_latency_ms` from sender and viewer relay-clock offsets, logs publisher capture-to-encode/send timing and relay receive-to-send queue delay, tracks packet loss from sequence gaps, records reassembly/decode/render timing percentiles and render FPS, periodically sends `ViewerStats` over the control stream, and sends control-plane keepalives while waiting for delayed media.

Run in separate terminals:

```bash
cargo run -p relay-server -- --listen 127.0.0.1:4433
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --media-run-ms 1000 --media-start-delay-ms 2000 --media-fps 5 --media-frame-bytes 800 --max-datagram-payload 700 --feedback-interval-frames 2
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --channel-name stage1 --media-run-ms 1000 --media-fps 5 --max-datagram-payload 700
```

For a visible preview, add `--render-output window` to the viewer command.
For a scripted remote-input control-plane check, add `--remote-input-script pointer-tap` to the screen viewer command. The viewer sends pointer press/release events after subscribing, and the broadcaster logs the queued input when it polls during the screen publishing loop. Keep the broadcaster at the default `--remote-input-output log` for automated smoke tests; `--remote-input-output native` intentionally injects remote input into the local Windows desktop with `SendInput`.

If the relay is started with `--access-token`, pass the same `--access-token` to both desktop-client commands.
If the relay is started with a custom `--max-datagram-payload`, use the same or lower `--max-datagram-payload` on the desktop broadcaster so packetized media stays below the relay ingress limit.
Small relay capacity limits such as `--max-rooms 1 --max-participants-per-room 2 --max-streams-per-room 1` are useful for boundary smoke tests; excess clients should receive control-plane errors instead of creating unbounded server state.

Expected behavior:

- The broadcaster prints five `media-send` lines at 5 fps for a 1000 ms run, including per-frame `capture_ms`, `encode_ms`, `packetize_ms`, and `send_ms`.
- Both clients print `time-sync-sample` lines and a selected `time-sync` line with the lowest observed relay RTT and `clock_offset_micros` after `Hello`. Use `--time-sync-samples` and `--time-sync-spacing-ms` to tune startup sampling.
- The broadcaster publishes `StreamConfig`, sets target bitrate/framerate, and the viewer polls config before media receive.
- Long `--media-start-delay-ms` and `--media-idle-timeout-ms` windows are kept alive with `Ping`/`Pong` control messages.
- The viewer receives, decodes, and renders five frames split across ten packets with `--max-datagram-payload 700`.
- Each decoded frame prints a `media-render` line with render timestamp, BGRA buffer size, decode time, render time, and render FPS; `--render-output window` also blits the frame into a native preview window.
- The final broadcaster summary includes capture/encode/packetize/send p50 and p95 timing.
- Each received frame prints `latency_ms`, `calibrated_latency_ms`, `sender_encode_ms`, `sender_send_ms`, `server_queue_ms`, and `reassembly_ms`, and the final viewer summary includes latest estimated and calibrated latency plus sender encode/send, server queue, reassembly, decode, and render p50/p95 timing.
- The viewer reassembly buffer drops stale incomplete frames after `--reassembly-window-frames`, and also drops oldest incomplete frames when `--jitter-buffer-max-ms` would be exceeded.
- The relay enforces each viewer/stream egress queue's media-time budget, dropping over-budget datagrams for that stream without blocking other viewers or the same viewer's other subscribed streams.
- With `--remote-input-script pointer-tap`, the viewer receives `RemoteInputQueued` responses and the broadcaster prints a `remote-input-batch`, `remote-input event=pointer-button`, and `remote-input-summary output=log applied=2` on the default log output path.
- The viewer sends periodic `ViewerStats` and receives `PublisherFeedback` responses.
- New subscribers, packet loss, and decoder recovery can register keyframe requests with the relay.
- The broadcaster polls aggregated `PublisherFeedback`; when feedback requests a keyframe, the synthetic encoder marks the next frame as a keyframe.
- The broadcaster polls relay `StreamMetrics` at the end of the run to report server-observed ingress, cumulative queued/dropped egress datagrams, current per-stream egress queue packet/media depth, and server route p50/p95 timing.
- When most viewers are degraded by packet loss, dropped frames, excessive jitter/latency, slow reassembly/decode/render p95, or low render FPS, relay feedback lowers the synthetic target bitrate, then framerate, then screen resolution when the earlier targets are already at their floor. The broadcaster shrinks subsequent synthetic frame payloads, rescales screen frames when needed, emits a keyframe after a resolution change, and updates `StreamConfig`.
- The viewer unsubscribes and leaves on normal exit; when the last participant leaves, the relay removes the empty room from subsequent discovery.
- The final viewer summary reports zero loss and drops on a healthy local run.

## Desktop synthetic voice checks

The same desktop client path can publish a synthetic Opus-like voice stream with `--media-kind voice`. The relay validates it as `MediaKind::Voice`, forwards it as audio datagrams, and the viewer reassembles, decodes, and plays frames into a latest-audio playback sink.

Run in separate terminals after starting the relay:

```bash
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --media-kind voice --media-run-ms 1000 --media-start-delay-ms 2000 --media-fps 50 --media-frame-bytes 96 --feedback-interval-frames 10
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --channel-name stage1 --media-kind voice --media-run-ms 1000 --media-fps 50
```

For audible local playback, add `--audio-output speaker` to the viewer command. The default `sink` mode keeps smoke tests quiet and records only the latest played-frame summary.

For TeamSpeak-style voice state checks, add `--muted` to the broadcaster, `--push-to-talk` without `--ptt-active` to simulate an idle talk key, or `--deafened` to the viewer. A muted or idle push-to-talk broadcaster updates relay voice state and publishes no voice packets; the relay also rejects stray voice datagrams from that inactive speaker state. A deafened viewer updates relay voice state, receives no voice datagrams for that stream, and exits the voice receive loop without opening playback.

To inspect the current room presence and voice state without subscribing to media, run:

```bash
cargo run -p desktop-client -- --list-rooms
cargo run -p desktop-client -- --list-streams --channel-name stage1
cargo run -p desktop-client -- --list-participants --channel-name stage1 --display-name Alice
```

Expected behavior:

- The broadcaster publishes an Opus voice stream config and prints `audio-send` lines with capture/encode/packetize/send timing.
- The viewer prints `audio-recv` and `audio-play` lines for each decoded frame, including estimated/calibrated latency, sender encode/send, server queue, reassembly, decode/play timing, and playback FPS.
- With `--audio-output speaker`, the viewer queues decoded PCM to the default Windows speaker through WinMM while keeping the same `audio-play` metrics.
- With `--push-to-talk --ptt-active`, the broadcaster prints `voice-state` with `speaking=true` and sends voice frames as usual.
- With `--deafened`, the viewer prints `voice-state`, `voice-deafened`, and a zero-frame voice summary instead of `audio-play`.
- `--list-rooms` prints one `room` line per visible room; `--list-streams` joins the selected room, prints one `stream` line per published stream, and leaves.
- `--list-participants` prints one `participant` line per room member with `display_name`, `muted`, `deafened`, `push_to_talk`, `speaking`, `published_streams`, and `subscribed_streams`.
- The broadcaster polls relay `StreamMetrics`; a healthy single-viewer run reports queued egress datagrams, zero drops, current stream egress queue depth, and server route timing percentiles.
- The final viewer summary reports `kind=voice`, matching decoded and played frame counts, and zero loss on a healthy local run.

## Desktop dual-stream broadcaster/viewer checks

The broadcaster can publish channel screen sharing and channel voice together from one QUIC connection with `--channel-name` and `--media-kind both`. The screen stream uses `--stream-id`; the voice stream uses `--voice-stream-id` or defaults to the next stream id. A viewer can subscribe to both selected streams from one process and demux forwarded media packets by stream id.

Run in separate terminals after starting the relay:

```bash
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --channel-name stage1 --media-kind both --stream-id 1 --voice-stream-id 2 --media-run-ms 1000 --media-start-delay-ms 2000 --media-fps 30 --media-frame-bytes 800 --feedback-interval-frames 10
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --channel-name stage1 --media-kind both --stream-id 1 --voice-stream-id 2 --media-run-ms 1000 --media-fps 30
```

Expected behavior:

- The broadcaster publishes and configures two streams, one `Screen/H264` and one `Voice/Opus`.
- The broadcaster prints both `media-send` and `audio-send` lines during the same run.
- The viewer subscribes to both stream ids, prints both `media-render` and `audio-play` lines, sends per-stream viewer stats, and reports separate zero-loss screen and voice summaries on a healthy local run.

## Desktop microphone voice checks

On Windows, the broadcaster can use a real microphone source instead of synthetic samples. The payload is still the temporary Opus-like test container, but it now carries captured PCM so the receive/playback path sees the original microphone samples.

Run in separate terminals after starting the relay and choosing an id from `--list-audio-sources`:

```bash
cargo run -p desktop-client -- --mode broadcaster --relay 127.0.0.1:4433 --channel-name stage1 --media-kind voice --voice-input microphone --microphone-id 0 --media-run-ms 1000 --media-start-delay-ms 2000 --media-fps 50 --feedback-interval-frames 10
cargo run -p desktop-client -- --mode viewer --relay 127.0.0.1:4433 --channel-name stage1 --media-kind voice --media-run-ms 1000 --media-fps 50
```

Add `--audio-output speaker` to the viewer to hear the decoded microphone PCM through the default Windows speaker.

Expected behavior:

- The broadcaster prints `audio-send` lines with `voice_input=Microphone`.
- The viewer prints `audio-play` lines whose sample count matches the microphone frame duration, usually 960 samples per channel at 48 kHz and 50 fps.
- The final summaries should still report matching sent, decoded, and played voice frames on a healthy local run.

## Measurement plan

Early milestones measure synthetic packet forwarding latency, queue behavior, encoded-frame reassembly behavior, local jitter-budget frame drops, remote-input control-plane queueing and optional native broadcaster-side injection, capture queue behavior, live primary-monitor acquisition, Media Foundation H.264 encoder/decoder backend probing, microphone PCM capture handoff, optional speaker playback handoff, mute/deafen/push-to-talk voice-state control, participant presence discovery, dual-stream broadcaster publication and viewer demux, synthetic QUIC forwarding behavior, synthetic voice forwarding behavior, microphone voice forwarding behavior, synthetic capture-to-viewer latency, multi-sample startup relay clock offset estimates, runtime TimeSync refreshes, TimeSync-derived calibrated capture-to-viewer latency, broadcaster capture/encode/packetize/send timing, publisher stamped capture-to-encode/send timing, server receive-to-route timing, relay receive-to-send queue timing, viewer receive-to-reassembly timing, viewer decode/render timing, and render/playback FPS. Later milestones add hardware frame submission/output, real Opus, native viewer input capture, more advanced cross-machine clock offset filtering, viewer receive, decode, and render timestamp calibration.

High-speed camera validation should be used to calibrate in-app estimates once live rendering exists.
