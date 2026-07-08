# Protocol

The protocol is split into a reliable control plane and an unreliable low-latency media plane.

## Control plane

Control messages travel over reliable QUIC streams. They cover room creation, joins, publishing, subscription, keyframe requests, stream configuration, and media feedback.

## Media plane

Media packets travel over QUIC datagrams. Each datagram carries one packet with a versioned binary header and an opaque encoded payload.

Encoded frames may be fragmented into multiple datagrams for MTU safety. Fragmentation is only a transport concern; the relay never decodes or transforms video content.
