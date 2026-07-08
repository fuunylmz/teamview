#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportMode {
    QuicDatagram,
    ReliableControlStream,
}
