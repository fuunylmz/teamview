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
- placeholder authentication
- create room
- join room
- publish stream
- subscribe / unsubscribe stream
- leave room
- keyframe request
- stream config
- publisher feedback
- viewer stats
- target bitrate/framerate updates

JSON-line framing is intentionally a Stage 1 choice. Later stages can replace it with a compact binary serializer or length-prefixed binary envelope without changing the room/control state machine.

Keyframe requests are accepted from subscribed viewers and are also registered automatically when a viewer first subscribes to a stream. The relay exposes those requests to the publisher through `PublisherFeedback.keyframe_requested`; the publisher consumes the pending request when it polls feedback and should make the next encoded video frame a keyframe.

Publishers can set their current target bitrate and framerate. `PublisherFeedback` returns the relay's current bitrate/FPS target; if most subscribed viewers report degraded stats, the relay lowers the bitrate target before returning feedback so the publisher can adapt future encoded frames.

## Media plane

Media packets travel over QUIC datagrams. Each datagram carries one packet with a versioned binary header and an opaque encoded payload.

Encoded frames may be fragmented into multiple datagrams for MTU safety. Fragmentation is only a transport concern; the relay never decodes or transforms video content.

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
- `codec`
- `is_keyframe`
- opaque encoded bytes

`packetize_frame` splits a frame into `MediaPacket` fragments:

- `sequence_number` increments for every fragment.
- `frame_id` stays the same for all fragments in the frame.
- `fragment_index` and `fragment_count` describe reassembly order.
- `KEYFRAME` is set on fragments belonging to a keyframe.
- `END_OF_FRAME` is set only on the last fragment.

`reassemble_frame` sorts fragments by index, verifies frame metadata consistency, and reconstructs the original encoded bytes. Incomplete frames are rejected instead of being passed to a decoder.
