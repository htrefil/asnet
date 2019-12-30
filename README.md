# asnet
asnet is a simple asynchronous, packet-oriented networking library built on TCP.  
It uses the mio crate internally to create an event loop for multiplexing connections.
The API is inspired by the ENet networking library.

The library is useful for singlethreaded programs that don't want to use threading to work with networking.

## Usage
The core of the library revolves around the `Host` structure which encapsulates all peers.  
A peer represents a connection to a remote server or client. 

See the [examples](examples) directory for example usage.

## License
[MIT](LICENSE)