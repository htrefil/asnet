use mio::net::TcpStream;
use mio::Ready;
use std::cell::{Ref, RefCell, RefMut};
use std::collections::VecDeque;
use std::io::{Error, ErrorKind, Read, Write};
use std::net::SocketAddr;
use std::rc::Rc;
use std::time::Instant;

/// Represents the state of a peer in a host.
pub struct Peer<T> {
    shared: Rc<RefCell<Shared<T>>>,
}

impl<T> Peer<T>
where
    T: Default,
{
    pub(crate) fn new(stream: TcpStream, addr: SocketAddr, state: ConnectionState) -> Peer<T> {
        Peer {
            shared: Rc::new(RefCell::new(Shared {
                data: Default::default(),
                stream: stream,
                addr: addr,
                incoming_packets: VecDeque::new(),
                outgoing_packets: VecDeque::new(),
                ready: Ready::empty(),
                read_state: None,
                write_state: None,
                last_activity: Instant::now(),
                connection_state: state,
            })),
        }
    }

    pub(crate) fn update(&self) -> Result<(), Error> {
        self.shared.borrow_mut().update()
    }

    pub(crate) fn ready(&self, ready: Ready) {
        self.shared.borrow_mut().ready.insert(ready);
    }

    pub(crate) fn connection_state(&self) -> ConnectionState {
        self.shared.borrow().connection_state
    }

    pub(crate) fn set_connection_state(&self, state: ConnectionState) {
        self.shared.borrow_mut().connection_state = state;
    }

    pub(crate) fn last_activity(&self) -> Instant {
        self.shared.borrow().last_activity
    }

    pub(crate) fn incoming_packets<'a>(&'a self) -> IncomingPackets<'a> {
        IncomingPackets {
            packets: RefMut::map(self.shared.borrow_mut(), |shared| {
                &mut shared.incoming_packets
            }),
        }
    }

    /// Disconnects the peer.
    /// If not already disconnected, this will generate a Disconnect event.
    pub fn disconnect(&self) {
        self.shared.borrow_mut().connection_state = ConnectionState::Disconnected;
    }

    /// Queues a packet to be sent.
    ///
    /// Doing this operation on a disconnected peer has no effect.
    pub fn send(&self, packet: Vec<u8>) {
        self.shared.borrow_mut().outgoing_packets.push_back(packet);
    }

    /// Returns a reference to its data.
    /// Panics if there is a mutable reference to the data.
    pub fn data(&self) -> Ref<T> {
        Ref::map(self.shared.borrow(), |shared| &shared.data)
    }

    /// Returns a mutable reference to its data.
    /// Panics if there are other reference(s) to the data.
    pub fn data_mut(&self) -> RefMut<T> {
        RefMut::map(self.shared.borrow_mut(), |shared| &mut shared.data)
    }

    /// Returns the socket address of the remote side.
    pub fn addr(&self) -> SocketAddr {
        self.shared.borrow().addr
    }
}

impl<T> Clone for Peer<T> {
    fn clone(&self) -> Peer<T> {
        Peer {
            shared: self.shared.clone(),
        }
    }
}

struct Shared<T> {
    data: T,
    stream: TcpStream,
    addr: SocketAddr,
    incoming_packets: VecDeque<Vec<u8>>,
    outgoing_packets: VecDeque<Vec<u8>>,
    ready: Ready,
    read_state: Option<ReadState>,
    write_state: Option<WriteState>,
    last_activity: Instant,
    connection_state: ConnectionState,
}

impl<T> Shared<T> {
    fn update(&mut self) -> Result<(), Error> {
        if self.ready.is_readable() {
            self.readable()?;
        }

        if self.ready.is_writable() {
            self.writable()?;
        }

        Ok(())
    }

    fn readable(&mut self) -> Result<(), Error> {
        let mut processed = 0usize;

        loop {
            let mut buffer = [0u8; 512];
            let n = match self.stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                Err(ref err) if err.kind() == ErrorKind::WouldBlock => {
                    self.ready.remove(Ready::readable());
                    break;
                }
                Err(err) => return Err(err),
            };

            for e in buffer[0..n].iter().cloned() {
                self.read_state = match self.read_state.take() {
                    Some(ReadState::Size1(a)) => Some(ReadState::Size2(a, e)),
                    Some(ReadState::Size2(a, b)) => Some(ReadState::Size3(a, b, e)),
                    Some(ReadState::Size3(a, b, c)) => {
                        let size = u32::from_be_bytes([a, b, c, e]);
                        if size == 0 {
                            return Err(ErrorKind::InvalidData.into());
                        }

                        Some(ReadState::Packet(Vec::new(), size as usize))
                    }
                    Some(ReadState::Packet(mut packet, size)) => {
                        packet.push(e);
                        if packet.len() == size {
                            self.incoming_packets.push_back(packet);
                            None
                        } else {
                            Some(ReadState::Packet(packet, size))
                        }
                    }
                    None => Some(ReadState::Size1(e)),
                };
            }

            processed += n;
        }

        if processed != 0 {
            self.last_activity = Instant::now();
        }

        Ok(())
    }

    fn writable(&mut self) -> Result<(), Error> {
        let mut processed = 0usize;

        loop {
            let write_state = match self.write_state.take() {
                Some(write_state) => {
                    if write_state.done == write_state.data.len() {
                        continue;
                    }

                    write_state
                }
                None => {
                    let mut data = match self.outgoing_packets.pop_front() {
                        Some(data) => data,
                        None => break,
                    };

                    for (i, b) in (data.len() as u32)
                        .to_be_bytes()
                        .iter()
                        .cloned()
                        .enumerate()
                    {
                        data.insert(i, b);
                    }

                    WriteState {
                        data: data,
                        done: 0,
                    }
                }
            };

            let n = match self.stream.write(&write_state.data[write_state.done..]) {
                Ok(0) => break,
                Ok(n) => n,
                Err(ref err) if err.kind() == ErrorKind::WouldBlock => {
                    self.ready.remove(Ready::writable());
                    break;
                }
                Err(err) => return Err(err),
            };

            processed += n;
        }

        if processed != 0 {
            self.last_activity = Instant::now();
        }

        Ok(())
    }
}

enum ReadState {
    Size1(u8),
    Size2(u8, u8),
    Size3(u8, u8, u8),
    Packet(Vec<u8>, usize),
}

struct WriteState {
    data: Vec<u8>,
    done: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConnectionState {
    Waiting,
    Connected,
    Disconnected,
}

pub(crate) struct IncomingPackets<'a> {
    packets: RefMut<'a, VecDeque<Vec<u8>>>,
}

impl<'a> Iterator for IncomingPackets<'a> {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Vec<u8>> {
        self.packets.pop_front()
    }
}
