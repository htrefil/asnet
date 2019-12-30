use crate::event::{Event, EventKind};
use crate::peer::{ConnectionState, Peer};
use mio::net::{TcpListener, TcpStream};
use mio::{Events, Poll, PollOpt, Ready, Token};
use slab::Slab;
use std::collections::VecDeque;
use std::io::{Error, ErrorKind};
use std::marker::PhantomData;
use std::net::{Ipv4Addr, ToSocketAddrs};
use std::time::Duration;

/// The Host structure representing all connections.
pub struct Host<T> {
    listener: Option<TcpListener>,
    poll: Poll,
    poll_events: Events,
    events: VecDeque<Event<T>>,
    peers: Slab<Peer<T>>,
    timeout: Duration,
}

impl<T> Host<T>
where
    T: Default,
{
    /// Creates a builder for the Host structure.
    pub fn builder() -> Builder<T> {
        Builder::default()
    }

    /// Processes all incoming and outgoing data.
    /// This is the only place where actual IO happens.
    pub fn process(&mut self, timeout: Option<Duration>) -> Result<Option<Event<T>>, Error> {
        if let Some(event) = self.events.pop_front() {
            return Ok(Some(event));
        }

        // Remove disconnected and timed out peers, wake them up and collect incoming packets, if any.
        for i in 0..self.peers.len() {
            let peer = match self.peers.get(i) {
                Some(peer) => peer,
                None => continue,
            };

            if peer.connection_state() == ConnectionState::Disconnected
                || peer.last_activity().elapsed() > self.timeout
            {
                self.events.push_back(Event {
                    kind: EventKind::Disconnect,
                    peer: peer.clone(),
                });
                self.peers.remove(i);
                continue;
            }

            peer.update()?;
            for packet in peer.incoming_packets() {
                self.events.push_back(Event {
                    kind: EventKind::Receive(packet),
                    peer: peer.clone(),
                });
            }
        }

        self.poll.poll(&mut self.poll_events, timeout)?;
        for event in &self.poll_events {
            if event.token() == Token(0) {
                let listener = self.listener.as_mut().unwrap();
                let (stream, addr) = listener.accept()?;

                let entry = self.peers.vacant_entry();

                self.poll.register(
                    &stream,
                    Token(entry.key() + 1),
                    Ready::all(),
                    PollOpt::edge(),
                )?;

                let peer = Peer::new(stream, addr, ConnectionState::Connected);
                entry.insert(peer.clone());

                self.events.push_back(Event {
                    kind: EventKind::Connect,
                    peer: peer,
                });
                continue;
            }

            let peer = match self.peers.get(event.token().0 - 1) {
                Some(peer) => peer,
                None => continue,
            };

            if peer.connection_state() == ConnectionState::Waiting {
                peer.set_connection_state(ConnectionState::Connected);
                self.events.push_back(Event {
                    kind: EventKind::Connect,
                    peer: peer.clone(),
                });
            }

            peer.ready(event.readiness());
        }

        Ok(None)
    }

    /// Connects to a remote asnet server.
    /// If connection succeeds, the peer will generate a Connect event.
    pub fn connect(&mut self, addr: impl ToSocketAddrs) -> Result<Peer<T>, Error> {
        let addr = addr
            .to_socket_addrs()?
            .next()
            .ok_or_else::<Error, _>(|| ErrorKind::NotFound.into())?;
        let stream = TcpStream::connect(&addr)?;
        let entry = self.peers.vacant_entry();

        self.poll.register(
            &stream,
            Token(entry.key() + 1),
            Ready::all(),
            PollOpt::edge(),
        )?;

        let peer = Peer::new(stream, addr, ConnectionState::Waiting);
        entry.insert(peer.clone());

        Ok(peer)
    }
}

/// The builder for the Host structure.
pub struct Builder<T> {
    timeout: Duration,
    events_capacity: usize,
    data: PhantomData<T>,
}

impl<T> Default for Builder<T> {
    fn default() -> Builder<T> {
        Builder {
            timeout: Duration::from_secs(5),
            events_capacity: 256,
            data: PhantomData,
        }
    }
}

impl<T> Builder<T> {
    /// Sets the maximum time of inactivity (that means no packet sent and received) after which the peer will be disconnected.
    ///
    /// The default is 5 seconds.
    pub fn timeout(mut self, timeout: Duration) -> Builder<T> {
        self.timeout = timeout;
        self
    }

    /// Sets capacity for mio events.
    ///
    /// The default is 256.
    pub fn events_capacity(mut self, events_capacity: usize) -> Builder<T> {
        self.events_capacity = events_capacity;
        self
    }

    /// Creates a client host.
    pub fn client(self) -> Result<Host<T>, Error> {
        Ok(Host {
            listener: None,
            poll: Poll::new()?,
            poll_events: Events::with_capacity(self.events_capacity),
            events: VecDeque::new(),
            peers: Slab::new(),
            timeout: self.timeout,
        })
    }

    /// Creates a server host.
    pub fn server(self, port: u16) -> Result<Host<T>, Error> {
        let listener = TcpListener::bind(&(Ipv4Addr::LOCALHOST, port).into())?;
        let poll = Poll::new()?;
        poll.register(&listener, Token(0), Ready::all(), PollOpt::edge())?;

        Ok(Host {
            listener: Some(listener),
            poll: poll,
            poll_events: Events::with_capacity(self.events_capacity),
            events: VecDeque::new(),
            peers: Slab::new(),
            timeout: self.timeout,
        })
    }
}
