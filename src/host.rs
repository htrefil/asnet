use super::event::{Event, EventKind};
use super::peer::Peer;
use mio::net::{TcpListener, TcpStream};
use mio::{Events, Poll, PollOpt, Ready, Token};
use slab::Slab;
use std::collections::VecDeque;
use std::io::{Error, ErrorKind};
use std::marker::PhantomData;
use std::net::{SocketAddr, ToSocketAddrs};
use std::ops::{Index, IndexMut};
use std::time::{Duration, Instant};

/// The host structure representing all connections.
pub struct Host<T> {
    listener: Option<TcpListener>,
    poll: Poll,
    poll_events: Events,
    timeout: Duration,
    events: VecDeque<HostEvent>,
    peers: Slab<Peer<T>>,
    remove: Option<usize>,
}

impl<T> Host<T>
where
    T: Default,
{
    /// Creates a `HostBuilder`.
    ///
    /// Convenience method.
    pub fn builder() -> HostBuilder<T> {
        HostBuilder::default()
    }

    /// Returns a reference to a peer associated with this index, None if the index is invalid.
    pub fn peer(&self, idx: usize) -> Option<&Peer<T>> {
        self.peers.get(idx)
    }

    /// Returns a mutable reference to a peer associated with this index, None if the index is invalid.
    pub fn peer_mut(&mut self, idx: usize) -> Option<&mut Peer<T>> {
        self.peers.get_mut(idx)
    }

    /// Returns an iterator over all connected peers and their indices.
    pub fn peers(&self) -> impl Iterator<Item = (usize, &Peer<T>)> {
        self.peers.iter().filter(|(_, peer)| peer.connected())
    }

    /// Returns an iterator over all connected peers and their indices.
    pub fn peers_mut(&mut self) -> impl Iterator<Item = (usize, &mut Peer<T>)> {
        self.peers.iter_mut().filter(|(_, peer)| peer.connected())
    }

    /// Connects to a remote asnet server.
    ///
    /// Ifthis function succeeds, a `Connect` event will be always generated, however, if the remote side declines the connection,
    /// a `Disconnect` even will be generated immediately after that.
    pub fn connect<'a>(&'a mut self, addr: impl ToSocketAddrs) -> Result<&'a mut Peer<T>, Error> {
        let addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| ErrorKind::NotFound)?;
        let stream = match TcpStream::connect(&addr) {
            Ok(stream) => Some(stream),
            Err(err) => {
                if err.kind() != ErrorKind::ConnectionRefused {
                    return Err(err);
                }

                None
            }
        };

        let entry = self.peers.vacant_entry();
        let idx = entry.key();

        self.events.push_back(HostEvent {
            kind: EventKind::Connect,
            peer: idx,
        });

        if let Some(ref stream) = stream {
            self.poll.register(
                stream,
                Token(entry.key() + 1),
                Ready::all(),
                PollOpt::edge(),
            )?;
        } else {
            self.events.push_back(HostEvent {
                kind: EventKind::Disconnect,
                peer: idx,
            });
        }

        entry.insert(Peer::new(addr, stream, idx));
        Ok(&mut self.peers[idx])
    }

    /// Broadcasts a packet to all connected peers.
    ///
    /// Convenience method.
    pub fn broadcast(&mut self, packet: Vec<u8>) {
        let mut remaining = self.peers.len();
        for (_, peer) in &mut self.peers {
            remaining -= 1;

            if remaining == 0 {
                peer.send(packet);
                return;
            }

            peer.send(packet.clone());
        }
    }

    fn process_internal(&mut self, timeout: Duration) -> Result<(), Error> {
        let now = Instant::now();
        // Wake up peers and collect incoming packets.
        for (idx, peer) in self.peers.iter_mut() {
            if now - peer.last_activity() >= self.timeout {
                self.events.push_back(HostEvent {
                    kind: EventKind::Disconnect,
                    peer: idx,
                });
                continue;
            }

            if let Err(err) = peer.process() {
                match err.kind() {
                    ErrorKind::InvalidData
                    | ErrorKind::ConnectionRefused
                    | ErrorKind::ConnectionReset
                    | ErrorKind::ConnectionAborted
                    | ErrorKind::BrokenPipe => {
                        self.events.push_back(HostEvent {
                            kind: EventKind::Disconnect,
                            peer: idx,
                        });
                    }
                    _ => return Err(err),
                }
            }

            for packet in peer.incoming_packets() {
                self.events.push_back(HostEvent {
                    kind: EventKind::Receive(packet),
                    peer: idx,
                });
            }
        }

        self.poll.poll(&mut self.poll_events, Some(timeout))?;
        for event in &self.poll_events {
            if event.token() == Token(0) {
                let listener = self.listener.as_mut().unwrap();
                let (stream, addr) = match listener.accept() {
                    Ok((stream, addr)) => (stream, addr),
                    Err(err) => {
                        if err.kind() != ErrorKind::WouldBlock {
                            return Err(err);
                        }

                        continue;
                    }
                };
                let entry = self.peers.vacant_entry();
                let key = entry.key();

                self.poll
                    .register(&stream, Token(key + 1), Ready::all(), PollOpt::edge())?;

                entry.insert(Peer::new(addr, Some(stream), key));

                self.events.push_back(HostEvent {
                    kind: EventKind::Connect,
                    peer: key,
                });
                continue;
            }

            let peer = match self.peers.get_mut(event.token().0 - 1) {
                Some(peer) => peer,
                None => continue,
            };

            peer.update_ready(event.readiness());
        }

        Ok(())
    }

    fn pop_event(&mut self) -> Option<HostEvent> {
        if let Some(peer) = self.remove.take() {
            self.peers.remove(peer);
        }

        if let Some(event) = self.events.pop_front() {
            if event.kind == EventKind::Disconnect {
                self.remove = Some(event.peer);
            }

            return Some(event);
        }

        None
    }

    /// Sends outgoing packets and receives incoming packets. This is the only place where actual IO happens.
    ///
    /// Will block for maximum `timeout` duration of time.
    pub fn process<'a>(&'a mut self, timeout: Duration) -> Result<Option<Event<'a, T>>, Error> {
        if let Some(HostEvent { kind, peer }) = self.pop_event() {
            return Ok(Some(Event {
                kind,
                peer: &mut self.peers[peer],
            }));
        }

        self.process_internal(timeout)?;
        Ok(None)
    }

    /// Like `process`, but will block indefinitely until an event happens.
    pub fn process_blocking<'a>(&'a mut self) -> Result<Event<'a, T>, Error> {
        loop {
            if let Some(HostEvent { kind, peer }) = self.pop_event() {
                return Ok(Event {
                    kind,
                    peer: &mut self.peers[peer],
                });
            }

            self.process_internal(self.timeout)?;
        }
    }
}

impl<T> Index<usize> for Host<T> {
    type Output = Peer<T>;

    /// Returns a reference to a peer associated with this index.
    ///
    /// Panics if no such peer exists.
    fn index(&self, idx: usize) -> &Peer<T> {
        &self.peers[idx]
    }
}

impl<T> IndexMut<usize> for Host<T> {
    /// Returns a mutable reference to a peer associated with this index.
    ///
    /// Panics if no such peer exists.
    fn index_mut(&mut self, idx: usize) -> &mut Peer<T> {
        &mut self.peers[idx]
    }
}

/// The builder for the `Host` structure.
#[derive(Clone, Copy, Debug)]
pub struct HostBuilder<T> {
    events_capacity: usize,
    timeout: Duration,
    data: PhantomData<T>,
}

impl<T> HostBuilder<T> {
    /// Sets the maximum time of inactivity (that means no packets sent and received) after which the peer will be disconnected.
    ///
    /// The default is 5 seconds.
    pub fn timeout(mut self, timeout: Duration) -> HostBuilder<T> {
        self.timeout = timeout;
        self
    }

    /// Sets capacity for mio events.
    ///
    /// The default is 256.
    pub fn events_capacity(mut self, events_capacity: usize) -> HostBuilder<T> {
        self.events_capacity = events_capacity;
        self
    }

    /// Creates a client host.
    pub fn client(self) -> Result<Host<T>, Error> {
        Ok(Host {
            listener: None,
            poll: Poll::new()?,
            poll_events: Events::with_capacity(self.events_capacity),
            timeout: self.timeout,
            events: VecDeque::new(),
            peers: Slab::new(),
            remove: None,
        })
    }

    /// Creates a server host.
    pub fn server(self, addr: SocketAddr) -> Result<Host<T>, Error> {
        let listener = TcpListener::bind(&addr)?;
        let poll = Poll::new()?;
        poll.register(&listener, Token(0), Ready::all(), PollOpt::edge())?;

        Ok(Host {
            listener: Some(listener),
            poll,
            poll_events: Events::with_capacity(self.events_capacity),
            timeout: self.timeout,
            events: VecDeque::new(),
            peers: Slab::new(),
            remove: None,
        })
    }
}

impl<T> Default for HostBuilder<T> {
    fn default() -> HostBuilder<T> {
        HostBuilder {
            events_capacity: 256,
            timeout: Duration::from_secs(5),
            data: PhantomData,
        }
    }
}

struct HostEvent {
    kind: EventKind,
    peer: usize,
}
