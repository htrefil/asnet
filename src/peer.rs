use mio::net::TcpStream;
use mio::Ready;
use std::collections::VecDeque;
use std::fmt::{self, Debug, Formatter};
use std::io::{Error, ErrorKind, Read, Write};
use std::net::SocketAddr;
use std::time::Instant;

/// The peer structure representing a connection to a remote endpoint.
pub struct Peer<T> {
    addr: SocketAddr,
    stream: Option<TcpStream>,
    ready: Ready,
    data: T,
    outgoing_packets: VecDeque<Vec<u8>>,
    incoming_packets: VecDeque<Vec<u8>>,
    write_state: Option<WriteState>,
    read_state: Option<ReadState>,
    last_activity: Instant,
    idx: usize,
}

impl<T> Peer<T>
where
    T: Default,
{
    pub(crate) fn new(addr: SocketAddr, stream: Option<TcpStream>, idx: usize) -> Peer<T> {
        Peer {
            addr,
            stream,
            ready: Ready::empty(),
            data: T::default(),
            outgoing_packets: VecDeque::new(),
            incoming_packets: VecDeque::new(),
            write_state: None,
            read_state: None,
            last_activity: Instant::now(),
            idx,
        }
    }

    pub(crate) fn connected(&self) -> bool {
        self.stream.is_some()
    }

    pub(crate) fn update_ready(&mut self, ready: Ready) {
        self.ready.insert(ready);
    }

    pub(crate) fn process(&mut self) -> Result<(), Error> {
        if self.ready.is_writable() {
            self.process_writable()?;
        }

        if self.ready.is_readable() {
            self.process_readable()?;
        }

        Ok(())
    }

    pub(crate) fn last_activity(&self) -> Instant {
        self.last_activity
    }

    fn process_writable(&mut self) -> Result<(), Error> {
        if let Some(ref mut stream) = self.stream {
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

                let n = match stream.write(&write_state.data[write_state.done..]) {
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
        }

        Ok(())
    }

    fn process_readable(&mut self) -> Result<(), Error> {
        if let Some(ref mut stream) = self.stream {
            let mut processed = 0usize;

            loop {
                let mut buffer = [0u8; 512];
                let n = match stream.read(&mut buffer) {
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
        }

        Ok(())
    }

    pub(crate) fn incoming_packets<'a>(&'a mut self) -> impl Iterator<Item = Vec<u8>> + 'a {
        self.incoming_packets.drain(0..)
    }

    /// Disconnects this peer.
    pub fn disconnect(&mut self) {
        self.stream = None;
    }

    /// Queues a packet to be sent.
    pub fn send(&mut self, packet: Vec<u8>) {
        self.outgoing_packets.push_back(packet);
    }

    /// Returns the socket address of the remote side.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Returns a reference to associated data.
    pub fn data(&self) -> &T {
        &self.data
    }

    /// Returns a mutable reference to associated data.
    pub fn data_mut(&mut self) -> &mut T {
        &mut self.data
    }

    /// Returns the index of this peer in the `Host` structure.
    pub fn idx(&self) -> usize {
        self.idx
    }
}

impl<T> Debug for Peer<T>
where
    T: Debug,
{
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        f.debug_struct("Peer")
            .field("addr", &self.addr)
            .field("data", &self.data)
            .field("idx", &self.idx)
            .finish()
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
