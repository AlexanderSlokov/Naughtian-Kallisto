use std::net::{SocketAddr, TcpListener};

use socket2::{Domain, Protocol, Socket, Type};

/// Creates a TCP listener bound to `addr` with SO_REUSEPORT enabled.
///
/// This is required to allow multiple Tokio worker threads (each running their
/// own single-threaded runtime) to listen on the same port concurrently,
/// matching Envoy's architecture for maximum throughput and avoiding a single
/// acceptor bottleneck. ADR-0016 QĐ-3 kept that architecture.
///
/// The address is a parameter rather than a hardcoded `0.0.0.0` because
/// ADR-0015's first operating constraint is that this port is not reachable
/// from the network. [`crate::config`] defaults it to loopback and refuses
/// anything else without an explicit acceptance of the risk; binding the
/// wildcard address here would have quietly overruled both.
pub fn bind_reuseport(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let domain = if addr.is_ipv6() {
        Domain::IPV6
    } else {
        Domain::IPV4
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;

    socket.set_reuse_port(true)?; // SO_REUSEPORT
    socket.set_reuse_address(true)?; // SO_REUSEADDR
    socket.set_nodelay(true)?; // TCP_NODELAY for lower latency
    socket.set_nonblocking(true)?;

    socket.bind(&addr.into())?;
    socket.listen(1024)?;

    Ok(socket.into())
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use super::*;

    #[test]
    fn it_binds_where_it_is_told_and_not_on_the_wildcard() {
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let listener = bind_reuseport(addr).unwrap();
        let bound = listener.local_addr().unwrap();
        assert_eq!(bound.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_ne!(bound.port(), 0);
    }
}
