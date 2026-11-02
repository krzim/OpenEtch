//! A blocking UDP channel to a controller (feature `udp`): stop-and-wait with ACK, resend on ERR/NAK, and the data
//! replies after memory reads. Transport only; no driver logic.
//!
//! A datagram whose ACK does not come is **not** sent again unless it only reads (ENQ, memory reads, document names):
//! the controller may have run it and only the ACK was lost, and sending moves and cuts twice would burn them twice
//! (UDP datagrams carry no sequence number). MeerK40t's client does the same: it resends on NAK only and treats a
//! missing ACK as a lost connection. After [`UdpChannel::tries`] waits the channel gives up with [`UdpError::NoAck`].

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::command::{Command, split};
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
    #[error("{0} memory reads or name requests in one payload; send them one at a time")]
    SeveralReplies(usize),
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
    /// Waits for an ACK (each one [`Self::timeout`] long, and each NAK/ERR resend counts as one) before giving up.
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

    /// Send one payload of whole commands (unscrambled, at most [`MAX_UDP_PAYLOAD`] bytes, at most one command that
    /// asks for data) and wait for the ACK, then for the data reply when the payload asks for one.
    pub fn send_payload(&mut self, plain: &[u8]) -> Result<Option<Reply>, UdpError> {
        let asking = split(plain).filter(|(_, c)| expects_data_reply(c)).count();
        if asking > 1 {
            return Err(UdpError::SeveralReplies(asking));
        }
        let packet = frame_udp(&self.swz, plain)?;
        let read_only = is_read_only(plain);
        let mut reply: Option<Reply> = None;
        let mut waits = 0;
        self.socket.send_to(&packet, self.controller)?;
        self.stats.sent += 1;
        'ack: loop {
            if waits >= self.tries {
                return Err(UdpError::NoAck(waits));
            }
            waits += 1;
            let deadline = Instant::now() + self.timeout;
            loop {
                match self.recv(deadline)? {
                    None => {
                        self.stats.timeouts += 1;
                        // Only a payload that changes nothing may go out again without an answer.
                        if read_only {
                            self.socket.send_to(&packet, self.controller)?;
                            self.stats.sent += 1;
                        }
                        continue 'ack;
                    }
                    Some(r) => match r.ack_action() {
                        AckAction::Proceed => {
                            self.stats.acks += 1;
                            break 'ack;
                        }
                        AckAction::Resend => {
                            // Refused (bad checksum, busy): the controller did not run it.
                            self.stats.resends += 1;
                            self.socket.send_to(&packet, self.controller)?;
                            self.stats.sent += 1;
                            continue 'ack;
                        }
                        // A data reply before the ACK is kept (MeerK40t's client forwards it too).
                        AckAction::KeepWaiting if asking == 1 && reply.is_none() && is_data(&r) => reply = Some(r),
                        AckAction::KeepWaiting => {}
                    },
                }
            }
        }
        if asking == 0 {
            return Ok(None);
        }
        if let Some(r) = reply {
            self.stats.replies += 1;
            return Ok(Some(r));
        }
        let deadline = Instant::now() + self.timeout * self.tries;
        loop {
            match self.recv(deadline)? {
                // A late duplicate ACK/ENQ is not the data.
                Some(r) if !is_data(&r) => {}
                Some(r) => {
                    self.stats.replies += 1;
                    return Ok(Some(r));
                }
                None => return Err(UdpError::NoReply),
            }
        }
    }

    /// Send an unscrambled command stream (e.g. a whole `.rd` job from [`crate::rd::write_rd_plain`]). Commands that
    /// ask for data each go in a datagram of their own, so every reply is matched to its request.
    pub fn send_stream(&mut self, plain: &[u8]) -> Result<Vec<Reply>, UdpError> {
        let mut replies = Vec::new();
        for p in packetize_for_replies(plain)? {
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

/// Whether sending `plain` twice changes nothing on the controller: only keepalives, memory reads and document-name
/// requests.
fn is_read_only(plain: &[u8]) -> bool {
    split(plain).all(|(_, c)| matches!(c, [0xCE] | [0xDA, 0x00, ..] | [0xE8, 0x01, ..]))
}

/// A reply that carries data (not a one-byte ACK/NAK/ERR/ENQ).
fn is_data(r: &Reply) -> bool {
    !matches!(r, Reply::Ack | Reply::Nak | Reply::Err | Reply::Enq)
}

/// [`packetize`], with every command that asks for data alone in its datagram.
fn packetize_for_replies(plain: &[u8]) -> Result<Vec<Vec<u8>>, UdpError> {
    let mut out = Vec::new();
    let mut run_start = 0;
    let flush = |out: &mut Vec<Vec<u8>>, run: &[u8]| -> Result<(), UdpError> {
        out.extend(packetize(run, MAX_UDP_PAYLOAD)?);
        Ok(())
    };
    for (off, c) in split(plain) {
        if expects_data_reply(c) {
            flush(&mut out, &plain[run_start..off])?;
            flush(&mut out, c)?;
            run_start = off + c.len();
        }
    }
    flush(&mut out, &plain[run_start..])?;
    Ok(out)
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
    fn several_reads_each_get_their_reply() {
        let (addr, stop, h) = spawn(Simulator::new(0x88));
        let mut ch = UdpChannel::bind("127.0.0.1:0".parse().unwrap(), addr, 0x88).unwrap();
        // Before: both reads went in one datagram and the second reply was dropped.
        let replies = ch
            .send_commands(&[
                Command::GetMemory(mem::CARD_ID),
                Command::MoveAbs { x: 7, y: 8 },
                Command::GetMemory(mem::BED_SIZE_X),
                Command::GetMemory(mem::POSITION_X),
            ])
            .unwrap();
        let values: Vec<u64> = replies
            .iter()
            .map(|r| match r {
                Reply::Memory { value, .. } => *value,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(values, [mem::CARD_ID_RDC6442S, 600_000, 7]);
        assert!(matches!(
            ch.send_payload(&crate::command::encode_all(&[Command::GetMemory(mem::CARD_ID), Command::GetMemory(mem::CARD_ID)]).unwrap()),
            Err(UdpError::SeveralReplies(2))
        ));
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
    }

    #[test]
    fn a_lost_ack_does_not_cut_twice() {
        let mut sim = Simulator::new(0x88);
        sim.lose_next_acks = 1;
        let (addr, stop, h) = spawn(sim);
        let mut ch = UdpChannel::bind("127.0.0.1:0".parse().unwrap(), addr, 0x88).unwrap();
        ch.timeout = Duration::from_millis(50);
        ch.tries = 3;
        let cuts = [
            Command::Power { kind: crate::command::PowerKind::Max, laser: crate::command::Laser::L1, power: crate::num::Power::FULL },
            Command::MoveAbs { x: 0, y: 0 },
            Command::CutRelX { dx: 1000 },
        ];
        // The controller ran the datagram; only its ACK was lost. Before, the channel sent it again and the relative
        // cut ran twice (burning 0..2 mm instead of 0..1 mm).
        assert!(matches!(ch.send_commands(&cuts), Err(UdpError::NoAck(3))));
        // A read-only payload may go out again: the keepalive after a lost ACK still succeeds.
        stop.store(true, Ordering::Relaxed);
        let mut sim = h.join().unwrap();
        assert_eq!(sim.burns().count(), 1);
        assert_eq!(sim.pos[0], 1000);
        sim.lose_next_acks = 1;
        let (addr, stop, h) = spawn(sim);
        let mut ch = UdpChannel::bind("127.0.0.1:0".parse().unwrap(), addr, 0x88).unwrap();
        ch.timeout = Duration::from_millis(50);
        ch.keepalive().unwrap();
        assert_eq!(ch.stats.sent, 2);
        stop.store(true, Ordering::Relaxed);
        h.join().unwrap();
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
