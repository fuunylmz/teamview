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

## First milestone

The first milestone uses synthetic media datagrams instead of real capture. This proves the server routing model, per-viewer isolation, and low-latency queue policy before hardware capture and encoding are added.
