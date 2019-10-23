use super::event::{Event, EventKind};
use super::peer::{ConnectionState, Peer};
use mio::net::{TcpListener, TcpStream};
use mio::{Events, Poll, PollOpt, Ready, Token};
use slab::Slab;
use std::collections::VecDeque;
use std::io::{Error, ErrorKind};
use std::net::{Ipv4Addr, ToSocketAddrs};
use std::time::Duration;

const EVENTS_CAPACITY: usize = 256;
const TIMEOUT: Duration = Duration::from_secs(5);

pub struct Host<T> {
    listener: Option<TcpListener>,
    poll: Poll,
    poll_events: Events,
    events: VecDeque<Event<T>>,
    peers: Slab<Peer<T>>,
}

impl<T> Host<T>
where
    T: Default,
{
    pub fn client() -> Result<Host<T>, Error> {
        Ok(Host {
            listener: None,
            poll: Poll::new()?,
            poll_events: Events::with_capacity(EVENTS_CAPACITY),
            events: VecDeque::new(),
            peers: Slab::new(),
        })
    }

    pub fn server(port: u16) -> Result<Host<T>, Error> {
        let listener = TcpListener::bind(&(Ipv4Addr::LOCALHOST, port).into())?;
        let poll = Poll::new()?;
        poll.register(&listener, Token(0), Ready::all(), PollOpt::edge())?;

        Ok(Host {
            listener: Some(listener),
            poll: poll,
            poll_events: Events::with_capacity(EVENTS_CAPACITY),
            events: VecDeque::new(),
            peers: Slab::new(),
        })
    }

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
                || peer.last_activity().elapsed() > TIMEOUT
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
}
