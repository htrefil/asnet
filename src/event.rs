use super::peer::Peer;

#[derive(Clone)]
pub struct Event<T> {
    pub kind: EventKind,
    pub peer: Peer<T>,
}

#[derive(Clone, Debug)]
pub enum EventKind {
    Connect,
    Disconnect,
    Receive(Vec<u8>),
}
