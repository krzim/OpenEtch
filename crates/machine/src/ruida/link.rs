//! The byte transports of the Ruida driver: [`RuidaLink`] (one payload, stop-and-wait), the real controller over UDP
//! ([`UdpRuidaLink`], `ruida::udp::UdpChannel`), the simulator ([`SimRuida`], `ruida::sim::Simulator`) and no
//! controller at all ([`OfflineRuida`]). Like [`crate::Link`] they only move bytes; safety lives in
//! [`super::RuidaController`].

use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use ruida::frame::{AckAction, MAX_UDP_PAYLOAD, Reply, frame_udp};
use ruida::sim::Simulator;
use ruida::udp::{UdpChannel, UdpError};
use ruida::Swizzle;
use serde::Serialize;

use crate::link::LinkError;

/// One payload to a Ruida controller and its answer.
pub trait RuidaLink: Send {
    /// Send one payload (unscrambled whole commands, at most 1470 bytes, at most one that asks for data) and wait for
    /// its acknowledgement; returns the data reply when the payload asks for one (`None` when the controller
    /// acknowledged it but sent no data: a read it kept in a file being stored, a file it does not have). A payload
    /// that was not acknowledged is never sent again unless it only reads (the controller may have run it).
    fn send_payload(&mut self, plain: &[u8]) -> Result<Option<Reply>, LinkError>;
}

impl<L: RuidaLink + ?Sized> RuidaLink for Box<L> {
    fn send_payload(&mut self, plain: &[u8]) -> Result<Option<Reply>, LinkError> {
        (**self).send_payload(plain)
    }
}

/// No controller: every payload fails with [`LinkError::Disconnected`].
#[derive(Debug, Default)]
pub struct OfflineRuida;

impl RuidaLink for OfflineRuida {
    fn send_payload(&mut self, _plain: &[u8]) -> Result<Option<Reply>, LinkError> {
        Err(LinkError::Disconnected)
    }
}

/// A controller on the network (`ruida::udp`: host port 40200, controller port 50200, checksum, ACK, resend on
/// NAK/ERR only).
pub struct UdpRuidaLink {
    channel: UdpChannel,
}

impl std::fmt::Debug for UdpRuidaLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UdpRuidaLink").field("stats", &self.channel.stats).finish()
    }
}

fn udp_error(e: UdpError) -> LinkError {
    match e {
        UdpError::NoAck(n) => LinkError::Io(format!("the Ruida controller did not acknowledge after {n} tries (UDP)")),
        other => LinkError::Io(format!("Ruida UDP: {other}")),
    }
}

impl UdpRuidaLink {
    /// Talk to `address` (port 50200) with key `magic`, listening on port 40200.
    pub fn open(address: std::net::IpAddr, magic: u8) -> Result<Self, LinkError> {
        let channel = UdpChannel::connect(address, magic).map_err(|e| LinkError::Io(format!("Ruida UDP port 40200: {e}")))?;
        Ok(UdpRuidaLink { channel })
    }

    /// Explicit addresses (tests use loopback and free ports).
    pub fn bind(local: SocketAddr, controller: SocketAddr, magic: u8, timeout: Duration) -> Result<Self, LinkError> {
        let mut channel = UdpChannel::bind(local, controller, magic).map_err(|e| LinkError::Io(e.to_string()))?;
        channel.timeout = timeout;
        Ok(UdpRuidaLink { channel })
    }
}

impl RuidaLink for UdpRuidaLink {
    fn send_payload(&mut self, plain: &[u8]) -> Result<Option<Reply>, LinkError> {
        match self.channel.send_payload(plain) {
            // Acknowledged without data (as the simulator answers): a read the controller kept instead of answering
            // (it takes an upload into a file), or a file name it does not have.
            Err(UdpError::NoReply) => Ok(None),
            r => r.map_err(udp_error),
        }
    }
}

/// What the simulated controller saw, for the status page and for tests that prove what fired.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimRuidaReport {
    pub datagrams: usize,
    /// XY segments run (moves and cuts).
    pub segments: usize,
    /// Segments that would have fired the laser.
    pub burns: usize,
    pub jobs_finished: usize,
    /// Names of the stored files, in order.
    pub files: Vec<String>,
    pub running: bool,
    pub paused: bool,
    /// X, Y in µm.
    pub position_um: [i64; 2],
}

/// A shared view of a [`SimRuida`]'s controller.
#[derive(Debug, Clone)]
pub struct SimRuidaHandle(Arc<Mutex<Simulator>>);

impl SimRuidaHandle {
    pub fn report(&self) -> SimRuidaReport {
        let s = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        SimRuidaReport {
            datagrams: s.datagrams,
            segments: s.segments.len(),
            burns: s.burns().count(),
            jobs_finished: s.jobs_finished,
            files: s.documents.iter().map(|d| d.name.clone()).collect(),
            running: s.running,
            paused: s.paused,
            position_um: [s.pos[0], s.pos[1]],
        }
    }

    /// Run `f` on the simulator (tests: refuse or lose datagrams, inspect segments, store files directly).
    pub fn with<R>(&self, f: impl FnOnce(&mut Simulator) -> R) -> R {
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// The simulated controller behind the same framing as UDP (checksummed datagrams, ACK, NAK and ERR resends).
#[derive(Debug)]
pub struct SimRuida {
    sim: Arc<Mutex<Simulator>>,
    swz: Swizzle,
}

impl SimRuida {
    pub fn new(magic: u8) -> Self {
        SimRuida { sim: Arc::new(Mutex::new(Simulator::new(magic))), swz: Swizzle::new(magic) }
    }

    pub fn handle(&self) -> SimRuidaHandle {
        SimRuidaHandle(self.sim.clone())
    }
}

/// Only keepalives, memory reads and file-name requests may be sent again without an answer.
fn read_only(plain: &[u8]) -> bool {
    ruida::command::split(plain).all(|(_, c)| matches!(c, [0xCE] | [0xDA, 0x00, ..] | [0xE8, 0x01, ..]))
}

impl RuidaLink for SimRuida {
    fn send_payload(&mut self, plain: &[u8]) -> Result<Option<Reply>, LinkError> {
        if plain.is_empty() || plain.len() > MAX_UDP_PAYLOAD {
            return Err(LinkError::Io(format!("payload of {} bytes (1..={MAX_UDP_PAYLOAD})", plain.len())));
        }
        let packet = frame_udp(&self.swz, plain).map_err(|e| LinkError::Io(e.to_string()))?;
        for _ in 0..4 {
            let out = self.sim.lock().unwrap_or_else(PoisonError::into_inner).handle_udp(&packet);
            let mut replies = out.iter().map(|r| Reply::from_wire(&self.swz, r));
            match replies.next().map(|r| (r.ack_action(), r)) {
                Some((AckAction::Proceed, _)) => return Ok(replies.next()),
                Some((AckAction::Resend, _)) => {}
                // The ACK was lost: the controller may have run the payload.
                _ if read_only(plain) => {}
                _ => return Err(LinkError::Io("the Ruida controller did not acknowledge (simulated)".into())),
            }
        }
        Err(LinkError::Io("the Ruida controller refused the payload 4 times (simulated)".into()))
    }
}
