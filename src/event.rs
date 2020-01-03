use super::peer::Peer;

/// An event that occured on a particular peer.
#[derive(Debug)]
pub struct Event<'a, T> {
    pub kind: EventKind,
    pub peer: &'a mut Peer<T>,
}

/// The type of an event.
#[derive(Clone, Debug, PartialEq)]
pub enum EventKind {
    /// Peer was disconnected.
    Connect,
    /// Peer was disconnected.
    Disconnect,
    /// The remote sie of a peer has sent a packet.
    Receive(Vec<u8>),
}
