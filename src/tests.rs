use super::*;

use std::net::Ipv4Addr;
use std::sync::{Arc, Barrier};
use std::thread;

const PORT: u16 = 8000;

#[test]
fn test_packet_order() {
    const PACKETS: &[&[u8]] = &[b"\x01first", b"\x02second", b"\x03third", b"\x04fourth"];

    let barrier = Arc::new(Barrier::new(2));
    let handle = {
        let barrier = barrier.clone();
        thread::spawn(move || {
            let host = Host::<()>::builder().server((Ipv4Addr::LOCALHOST, PORT).into());

            barrier.wait();

            // Can't panic until we wait for the barrier otherwise the test will block indefinitely.
            let mut host = host.unwrap();

            let event = host.process_blocking().unwrap();
            assert_eq!(event.kind, EventKind::Connect);

            for packet in PACKETS {
                let event = host.process_blocking().unwrap();
                assert_eq!(event.kind, EventKind::Receive(packet.to_vec()));
            }

            let event = host.process_blocking().unwrap();
            assert_eq!(event.kind, EventKind::Disconnect);
        })
    };

    barrier.wait();

    let mut host = Host::<()>::builder().client().unwrap();
    let idx = host.connect((Ipv4Addr::LOCALHOST, PORT)).unwrap().idx();
    for packet in PACKETS {
        host[idx].send(packet.to_vec());
    }

    let event = host.process_blocking().unwrap();
    assert_eq!(event.kind, EventKind::Connect);

    let event = host.process_blocking().unwrap();
    assert_eq!(event.kind, EventKind::Disconnect);

    handle.join().unwrap();
}
