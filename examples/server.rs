use asnet::{Event, EventKind, Host};
use std::io::{Error, ErrorKind};
use std::time::Duration;

fn main() -> Result<(), Error> {
    // Create a host listening on port 8000 with timeout set to 1 second.
    let mut host = Host::<Option<String>>::builder()
        .timeout(Duration::from_secs(1))
        .server(8000)?;

    loop {
        // Check for events, the call will block for max 500 millis.
        let Event { kind, peer } = match host.process(Some(Duration::from_millis(500)))? {
            Some(event) => event,
            None => continue,
        };

        match kind {
            EventKind::Connect => println!("{} connected", peer.addr()),
            EventKind::Disconnect => {
                let who = peer
                    .data()
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| peer.addr().to_string());

                println!("{} disconnected", who);
            }
            EventKind::Receive(data) => {
                let who = peer
                    .data()
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| peer.addr().to_string());
                let name = String::from_utf8(data).map_err(|_| ErrorKind::InvalidData)?;

                println!("{} set his name to {}", who, name);
                *peer.data_mut() = Some(name);
            }
        }
    }
}
