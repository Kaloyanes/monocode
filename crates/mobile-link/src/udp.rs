//! Quinn's Tokio socket, except that a failed receive ends its endpoint.
//!
//! Quinn 0.11's own socket retries any receive error other than
//! `WouldBlock` at once, without yielding. Tokio keeps the socket marked
//! readable after such an error, so one that keeps failing (as iOS fails
//! every call on a socket it defuncted while the app was suspended) spins a
//! runtime thread at full speed in `recvmsg` for good: the task can't even be
//! aborted. Returning the error stops the endpoint's driver instead; its
//! connections then time out, and the next request opens a fresh socket.

use quinn::udp::{RecvMeta, Transmit, UdpSocketState};
use quinn::{AsyncUdpSocket, UdpPoller};
use std::{
    fmt, io,
    pin::Pin,
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::io::Interest;

/// A client endpoint, as `quinn::Endpoint::client`, on this socket.
pub fn endpoint(bind: std::net::SocketAddr) -> io::Result<quinn::Endpoint> {
    let socket = socket2::Socket::new(
        socket2::Domain::for_address(bind),
        socket2::Type::DGRAM,
        Some(socket2::Protocol::UDP),
    )?;
    if bind.is_ipv6() {
        // Dual-stack where allowed; an IPv6-only socket still reaches IPv6.
        let _ = socket.set_only_v6(false);
    }
    socket.bind(&bind.into())?;
    socket.set_nonblocking(true)?;
    quinn::Endpoint::new_with_abstract_socket(
        quinn::EndpointConfig::default(),
        None,
        Socket::wrap(socket.into())?,
        Arc::new(quinn::TokioRuntime),
    )
}

struct Socket {
    io: tokio::net::UdpSocket,
    state: UdpSocketState,
}

impl Socket {
    fn wrap(socket: std::net::UdpSocket) -> io::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            state: UdpSocketState::new((&socket).into())?,
            io: tokio::net::UdpSocket::from_std(socket)?,
        }))
    }
}

impl fmt::Debug for Socket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Socket").finish_non_exhaustive()
    }
}

impl AsyncUdpSocket for Socket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(Writable(self))
    }

    fn try_send(&self, transmit: &Transmit) -> io::Result<()> {
        self.io.try_io(Interest::WRITABLE, || {
            self.state.send((&self.io).into(), transmit)
        })
    }

    fn poll_recv(
        &self,
        cx: &mut Context,
        bufs: &mut [io::IoSliceMut<'_>],
        meta: &mut [RecvMeta],
    ) -> Poll<io::Result<usize>> {
        loop {
            ready!(self.io.poll_recv_ready(cx))?;
            match self.io.try_io(Interest::READABLE, || {
                self.state.recv((&self.io).into(), bufs, meta)
            }) {
                // Readiness was cleared: wait for the next datagram.
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => continue,
                result => return Poll::Ready(result),
            }
        }
    }

    fn local_addr(&self) -> io::Result<std::net::SocketAddr> {
        self.io.local_addr()
    }

    fn may_fragment(&self) -> bool {
        self.state.may_fragment()
    }

    fn max_transmit_segments(&self) -> usize {
        self.state.max_gso_segments()
    }

    fn max_receive_segments(&self) -> usize {
        self.state.gro_segments()
    }
}

struct Writable(Arc<Socket>);

impl fmt::Debug for Writable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Writable").finish_non_exhaustive()
    }
}

impl UdpPoller for Writable {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context) -> Poll<io::Result<()>> {
        self.0.io.poll_send_ready(cx)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::fd::AsRawFd;
    use std::time::Duration;

    /// A socket readable once and failing ever after must not hold its
    /// thread: the receive returns the error.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_socket_that_keeps_failing_returns_the_error_rather_than_spinning() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let address = socket.local_addr().unwrap();
        let fd = socket.as_raw_fd();
        let socket = Socket::wrap(socket).unwrap();
        // Readable, so Tokio holds the socket as ready to read.
        std::net::UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .send_to(b"x", address)
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        // As a defunct socket on iOS: every receive now fails, never with
        // WouldBlock.
        let null = std::fs::File::open("/dev/null").unwrap();
        assert_eq!(unsafe { dup2(null.as_raw_fd(), fd) }, fd);

        let (sent, received) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 2048];
            let mut bufs = [io::IoSliceMut::new(&mut buffer)];
            let mut meta = [RecvMeta::default()];
            let mut cx = Context::from_waker(std::task::Waker::noop());
            let result = socket.poll_recv(&mut cx, &mut bufs, &mut meta);
            let _ = sent.send(matches!(result, Poll::Ready(Err(_))));
        });
        let failed = received
            .recv_timeout(Duration::from_secs(2))
            .expect("the receive spun instead of returning");
        assert!(failed);
    }

    extern "C" {
        fn dup2(from: i32, to: i32) -> i32;
    }
}
