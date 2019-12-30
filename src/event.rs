use crate::peer::Peer;

/// An event that occured on a particular peer.
#[derive(Clone)]
pub struct Event<T> {
    pub kind: EventKind,
    pub peer: Peer<T>,
}

/// The type of an event.
#[derive(Clone, Debug)]
pub enum EventKind {
    /// Peer was connected.
    Connect,
    /// Peer was disconnected and will no longer generate any events.
    Disconnect,
    /// The remote side of a peer has sent a packet.
    Receive(Vec<u8>),
}
