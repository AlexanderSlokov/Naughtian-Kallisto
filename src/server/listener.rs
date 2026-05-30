use socket2::{Domain, Protocol, Socket, Type};
use std::net::TcpListener;

/// Creates a TCP listener bound to `0.0.0.0:{port}` with SO_REUSEPORT enabled.
/// 
/// This is required to allow multiple Tokio worker threads (each running their own
/// single-threaded runtime) to listen on the same port concurrently, matching Envoy's
/// architecture for maximum throughput and avoiding a single acceptor bottleneck.
pub fn bind_reuseport(port: u16) -> std::io::Result<TcpListener> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    
    socket.set_reuse_port(true)?;      // SO_REUSEPORT
    socket.set_reuse_address(true)?;   // SO_REUSEADDR
    socket.set_nodelay(true)?;         // TCP_NODELAY for lower latency
    socket.set_nonblocking(true)?;
    
    socket.bind(&format!("0.0.0.0:{}", port).parse::<std::net::SocketAddr>().unwrap().into())?;
    socket.listen(1024)?;
    
    Ok(socket.into())
}
