//! A blocking UDP channel to a controller (feature `udp`): stop-and-wait with ACK, resend on ERR/NAK or timeout,
//! and the data reply after memory reads. Transport only; no driver logic.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::command::Command;
use crate::frame::{AckAction, CONTROLLER_PORT, HOST_PORT, MAX_UDP_PAYLOAD, Reply, expects_data_reply, frame_udp, packetize};
use crate::mem::MemAddr;
use crate::swizzle::Swizzle;
use crate::RuidaError;

#[derive(Debug, thiserror::Error)]
pub enum UdpError {
    #[error("UDP: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Protocol(#[from] RuidaError),
    #[error("no ACK after {0} tries")]
    NoAck(u32),
    #[error("no data reply")]
    NoReply,
    #[error("unexpected reply {0:?}")]
    Unexpected(Reply),
}

/// Counters for diagnosis.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UdpStats {
    pub sent: u64,
    pub acks: u64,
    pub resends: u64,
    pub timeouts: u64,
    pub replies: u64,
}

pub struct UdpChannel {
    socket: UdpSocket,
    controller: SocketAddr,
    swz: Swizzle,
    /// Wait per try for the ACK or the data reply.
    pub timeout: Duration,
    /// Sends per datagram before giving up.
    pub tries: u32,
    pub stats: UdpStats,
}

impl UdpChannel {
    /// Listen on [`HOST_PORT`] and talk to `controller`:[`CONTROLLER_PORT`].
    pub fn connect(controller: IpAddr, magic: u8) -> io::Result<Self> {
        Self::bind(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), HOST_PORT),
            SocketAddr::new(controller, CONTROLLER_PORT),
            magic,
        )
    }

    /// Explicit addresses (tests use loopback and free ports).
    pub fn bind(local: SocketAddr, controller: SocketAddr, magic: u8) -> io::Result<Self> {
        let socket = UdpSocket::bind(local)?;
        Ok(UdpChannel {
            socket,
            controller,
            swz: Swizzle::new(magic),
            timeout: Duration::from_millis(250),
            tries: 4,
            stats: UdpStats::default(),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    fn recv(&mut self, deadline: Instant) -> Result<Option<Reply>, UdpError> {
        let mut buf = [0u8; 2048];
        loop {
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            self.socket.set_read_timeout(Some(deadline - now))?;
            match self.socket.recv_from(&mut buf) {
                Ok((n, from)) if from.ip() == self.controller.ip() => {
                    return Ok(Some(Reply::from_wire(&self.swz, &buf[..n])));
                }
                Ok(_) => {} // not from the controller
                Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => return Ok(None),
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// Send one payload of whole commands (unscrambled, at most [`MAX_UDP_PAYLOAD`] bytes) and wait for the ACK, then
    /// for the data reply when the payload asks for one.
    pub fn send_payload(&mut self, plain: &[u8]) -> Result<Option<Reply>, UdpError> {
        let packet = frame_udp(&self.swz, plain)?;
        let mut tries = 0;
        'send: loop {
            if tries >= self.tries {
                return Err(UdpError::NoAck(tries));
            }
            tries += 1;
            self.socket.send_to(&packet, self.controller)?;
            self.stats.sent += 1;
            let deadline = Instant::now() + self.timeout;
            loop {
                match self.recv(deadline)? {
                    None => {
                        self.stats.timeouts += 1;
                        continue 'send;
                    }
                    Some(r) => match r.ack_action() {
                        AckAction::Proceed => {
                            self.stats.acks += 1;
                            break 'send;
                        }
                        AckAction::Resend => {
                            self.stats.resends += 1;
                            continue 'send;
                        }
                        AckAction::KeepWaiting => {}
                    },
                }
            }
        }
        if !expects_data_reply(plain) {
            return Ok(None);
        }
        let deadline = Instant::now() + self.timeout * self.tries;
        match self.recv(deadline)? {
            Some(r) => {
                self.stats.replies += 1;
                Ok(Some(r))
            }
            None => Err(UdpError::NoReply),
        }
    }

    /// Send an unscrambled command stream (e.g. a whole `.rd` job from [`crate::rd::write_rd_plain`]).
    pub fn send_stream(&mut self, plain: &[u8]) -> Result<Vec<Reply>, UdpError> {
        let mut replies = Vec::new();
        for p in packetize(plain, MAX_UDP_PAYLOAD)? {
            replies.extend(self.send_payload(&p)?);
        }
        Ok(replies)
    }

    pub fn send_commands(&mut self, cmds: &[Command]) -> Result<Vec<Reply>, UdpError> {
        self.send_stream(&crate::command::encode_all(cmds)?)
    }

    /// ENQ; Ok when ACKed.
    pub fn keepalive(&mut self) -> Result<(), UdpError> {
        self.send_payload(&[0xCE]).map(|_| ())
    }

    pub fn read_memory(&mut self, addr: MemAddr) -> Result<u64, UdpError> {
        match self.send_payload(&Command::GetMemory(addr).encode()?)? {
            Some(Reply::Memory { addr: a, value, .. }) if a == addr => Ok(value),
            Some(other) => Err(UdpError::Unexpected(other)),
            None => Err(UdpError::NoReply),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;

    use super::*;
    use crate::mem;
    use crate::rd::{RdJob, RdLayer, WriteOptions, write_rd_plain};
    use crate::sim::Simulator;

    /// A simulator behind a loopback UDP socket; returns its address, a stop flag and the thread.
    fn spawn(mut sim: Simulator) -> (SocketAddr, Arc<AtomicBool>, thread::JoinHandle<Simulator>) {
        let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
        sock.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let addr = sock.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let h = thread::spawn(move || {
            let mut buf = [0u8; 2048];
            while !stop2.load(Ordering::Relaxed) {
                if let Ok((n, from)) = sock.recv_from(&mut buf) {
                    for r in sim.handle_udp(&buf[..n]) {
                        sock.send_to(&r, from).unwrap();
                    }
                }
            }
            sim
        });
        (addr, stop, h)
    }

    #[test]
    fn job_and_memory_over_loopback() {
        let mut sim = Simulator::new(0x88);
        sim.refuse_next = 2; // the first datagram is refused twice, then taken
        let (addr, stop, h) = spawn(sim);
        let mut ch = UdpChannel::bind("127.0.0.1:0".parse().unwrap(), addr, 0x88).unwrap();
        ch.keepalive().unwrap();
        assert_eq!(ch.stats.resends, 2);
        assert_eq!(ch.read_memory(mem::CARD_ID).unwrap(), mem::CARD_ID_RDC6442S);
        let polylines = (0..300).map(|i| vec![[f64::from(i), 0.0], [f64::from(i), 5.0]]).collect();
        let job = RdJob {
            layers: vec![RdLayer {
                color: [0, 0, 0],
                speed_mm_s: 100.0,
                min_power: [5.0, 5.0],
                max_power: [5.0, 5.0],
                frequency: None,
                polylines,
            }],
        };
        let plain = write_rd_plain(&job, &WriteOptions::default()).unwrap();
        assert!(plain.len() > MAX_UDP_PAYLOAD, "spans several datagrams");
        assert!(ch.send_stream(&plain).unwrap().is_empty());
        assert_eq!(mem::position_um(ch.read_memory(mem::POSITION_X).unwrap()), 299_000);
        stop.store(true, Ordering::Relaxed);
        let sim = h.join().unwrap();
        assert_eq!(sim.jobs_finished, 1);
        assert_eq!(sim.burns().count(), 300);
    }

    #[test]
    fn gives_up_without_controller() {
        let silent = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut ch = UdpChannel::bind("127.0.0.1:0".parse().unwrap(), silent.local_addr().unwrap(), 0x88).unwrap();
        ch.timeout = Duration::from_millis(10);
        ch.tries = 3;
        assert!(matches!(ch.keepalive(), Err(UdpError::NoAck(3))));
        assert_eq!(ch.stats.timeouts, 3);
    }
}
