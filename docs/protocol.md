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
