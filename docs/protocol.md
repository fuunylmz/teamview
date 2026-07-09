# Protocol

The protocol is split into a reliable control plane and an unreliable low-latency media plane.

## Control plane

Control messages travel over reliable QUIC bidirectional streams.

Stage 1 uses newline-delimited JSON envelopes so the control plane is easy to inspect while the transport and room model are still evolving.

Envelope shape:

```json
{
  "protocol_version": 1,
  "request_id": 7,
  "message": {
    "Hello": {
      "protocol_version": 1,
      "client_name": "desktop-client"
    }
  }
}
```

Every control envelope includes:

- `protocol_version`: currently `1`; unsupported versions are rejected.
- `request_id`: preserved by request/response handling for correlation.
- `message`: a `ClientControl` or `ServerControl` variant.

Current control messages cover:

- hello / hello accepted
- ping / pong keepalive
- optional shared-token authentication
- create room
- list rooms
- join room
- publish stream
- list streams
- subscribe / unsubscribe stream
- leave room
- keyframe request
- stream config
- stream metrics
- publisher feedback
- viewer stats
- target bitrate/framerate updates

JSON-line framing is intentionally a Stage 1 choice. Later stages can replace it with a compact binary serializer or length-prefixed binary envelope without changing the room/control state machine.

When the relay is started with an access token, clients must send `Authenticate` after `Hello` and before room, stream, stats, or media use. Invalid tokens return `invalid_token`; unauthenticated control actions return `not_authenticated`. Media datagrams from a connection that has not authenticated are dropped. Without an access token, `Hello` grants access for local development and tests.

Viewers can discover active sessions before subscribing. `ListRooms` returns room ids, names, participant counts, and published stream counts. After joining a room, `ListStreams` returns stream ids, publisher ids, codec/media kind, subscriber counts, config availability, and current target bitrate/FPS. The desktop viewer uses these messages to select a room by `--room-name` when `--room-id` is not provided.

Room creators are automatically added as participants. `LeaveRoom` removes the user from participants and subscriptions; if the leaving user published streams, those streams and their viewer stats, metrics, keyframe requests, and subscriptions are removed too. Empty rooms are removed from discovery. The desktop client sends `UnsubscribeStream` and `LeaveRoom` during normal viewer shutdown, sends `LeaveRoom` during normal broadcaster shutdown, and the relay applies the same cleanup when a connection disconnects unexpectedly.

Keyframe requests are accepted from subscribed viewers and are also registered automatically when a viewer first subscribes to a stream. The relay exposes those requests to the publisher through `PublisherFeedback.keyframe_requested`; the publisher consumes the pending request when it polls feedback and should make the next encoded video frame a keyframe.

Publishers can set their current target bitrate and framerate. `PublisherFeedback` returns the relay's current bitrate/FPS target; if most subscribed viewers report degraded stats, the relay lowers the bitrate target before returning feedback so the publisher can adapt future encoded frames.

`ViewerStats` carries packet counts, decoded/dropped frame counts, jitter, estimated capture-to-viewer latency, reassembly/decode/render p50 and p95 milliseconds, and render FPS. The relay treats packet loss, dropped frames, excessive jitter/latency, slow reassembly/decode/render p95, or low nonzero render FPS as degraded viewer signals for publisher feedback.

Room participants can poll `StreamMetrics` for a published stream. The relay reports server-observed ingress packets/bytes, cumulative queued and dropped egress datagrams, current egress queue packet/media depth, subscriber count, the last server ingress timestamp, and server route p50/p95 milliseconds from datagram receive to fanout enqueue/drop completion.

## Media plane

Media packets travel over QUIC datagrams. Each datagram carries one packet with a versioned binary header and an opaque encoded payload.

The relay uses the stream framerate and fragment count to estimate queued media time for each datagram. If forwarding a datagram would push a viewer beyond its configured egress media budget, only that viewer's datagram is dropped.

Encoded frames may be fragmented into multiple datagrams for MTU safety. Fragmentation is only a transport concern; the relay never decodes or transforms media content.

The packet header is defined in `crates/protocol/src/packet.rs` and includes:

- protocol version
- packet type
- flags
- header length
- room stream id
- sequence number
- frame id
- fragment index/count
- media timestamp
- sender capture timestamp
- sender encode-done timestamp
- sender send timestamp
- relay receive timestamp
- relay send timestamp
- codec id
- future layer id
- payload length

The decoder accepts longer header lengths for forward-compatible extensions, rejects shorter headers, validates fragment invariants, and rejects trailing bytes.

## Encoded frame packetization

Stage 3 adds reusable encoded-frame helpers in `crates/protocol/src/frame.rs`.

`EncodedFrame` represents one encoded access unit before network packetization:

- `room_stream_id`
- `frame_id`
- `media_timestamp`
- `sender_capture_time_micros`
- `sender_encode_done_time_micros`
- `sender_send_time_micros`
- `server_receive_time_micros`
- `server_send_time_micros`
- `codec`
- `is_keyframe`
- opaque encoded bytes

The desktop synthetic broadcaster writes `sender_capture_time_micros` as Unix epoch microseconds, stamps `sender_encode_done_time_micros` after encoding, and stamps `sender_send_time_micros` immediately before each QUIC datagram send. The relay stamps forwarded datagrams with `server_receive_time_micros` when it accepts an ingress datagram and `server_send_time_micros` immediately before it calls QUIC `send_datagram` for a viewer. Viewers compare sender capture/encode/send timestamps to log publisher-side media timing, compare relay receive/send timestamps to log server queue delay, and compare sender capture time with local receive time to populate `ViewerStats.estimated_latency_ms`; production cross-machine latency will need clock offset estimation before capture-to-viewer values can be treated as calibrated glass-to-glass latency.

`packetize_frame` splits a video frame into `MediaPacket` fragments. `packetize_frame_with_type` uses the same fragmentation rules for other media packet types such as audio:

- `sequence_number` increments for every fragment.
- `frame_id` stays the same for all fragments in the frame.
- `fragment_index` and `fragment_count` describe reassembly order.
- `KEYFRAME` is set on fragments belonging to a keyframe.
- `END_OF_FRAME` is set only on the last fragment.

`reassemble_frame` sorts fragments by index, verifies frame metadata consistency, reconstructs the original encoded bytes, carries the latest sender send timestamp across fragments, and carries the relay timestamp span from the earliest receive timestamp to the latest send timestamp. Incomplete frames are rejected instead of being passed to a decoder.
