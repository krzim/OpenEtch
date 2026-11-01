//! Transport framing.
//!
//! **UDP** (Ethernet controllers): the host sends to the controller's port [`CONTROLLER_PORT`] (50200) from, and
//! listens on, [`HOST_PORT`] (40200). Each datagram is a big-endian 16-bit checksum (the sum of the scrambled payload
//! bytes, mod 65536) followed by the scrambled payload: whole commands, at most [`MAX_UDP_PAYLOAD`] bytes. The
//! controller answers every datagram with one scrambled byte: ACK (`0xCC`, `0xC6` on the wire with key `0x88`) when it
//! took it, or ERR (`0xCD`, wire `0x46`) / NAK (`0xCF`, wire `0x48`) when it did not; the host then sends the same
//! datagram again. MeerK40t's emulator answers a bad checksum with NAK, and its client resends on NAK only (it ignores
//! ERR); which of the two a real controller sends is unverified, so this crate resends on both. A missing ACK is not a
//! refusal: the controller may have run the datagram, so only read-only datagrams are sent again after a timeout
//! (see [`crate::udp`]). It is stop-and-wait: one datagram in flight. Commands that ask for data (memory
//! reads, document names) are followed by a second datagram carrying the scrambled reply, without a checksum. The
//! controller sends nothing unprompted; a keepalive is a lone ENQ (`0xCE`), answered with ACK. Ports 50207/40207 carry
//! the `A5` jog-key interface (not covered here).
//!
//! **USB serial** (FTDI `0403:6001`, 8N1): the same scrambled command bytes with no checksum, no ACK/NAK and no reply
//! to ENQ, so a link check reads a memory register instead (MeerK40t reads [`crate::mem::MACHINE_STATUS`]).

// Facts from meerk40t/meerk40t @ d511944 `meerk40t/ruida/udp_connection.py` (`_package`, `_ruida_handshaker`, ports),
// `ruidasession.py` (`_package` per interface), `usb_transport.py` (module notes), `emulator.py` (`checksum_write`,
// the 1472-byte read, `msg_reply`) and `rdjob.py` (`ACK`, `NAK`, `ERR`, `ENQ`) (MIT License, Copyright (c) 2021
// meerk40t; full notice in `swizzle.rs`).

use serde::{Deserialize, Serialize};

use crate::command::{Command, split};
use crate::mem::MemAddr;
use crate::num::{decode_u14, decode_u35, encode_u14, encode_u35};
use crate::swizzle::Swizzle;
use crate::{Result, RuidaError};

/// The controller listens here (host → controller).
pub const CONTROLLER_PORT: u16 = 50200;
/// The host listens here (controller → host).
pub const HOST_PORT: u16 = 40200;
/// Jog-key interface, host → controller.
pub const JOG_CONTROLLER_PORT: u16 = 50207;
/// Jog-key interface, controller → host.
pub const JOG_HOST_PORT: u16 = 40207;
/// Payload bytes per datagram after the 2-byte checksum (MeerK40t's emulator reads at most 1472 bytes).
pub const MAX_UDP_PAYLOAD: usize = 1470;
/// USB serial adapter of known Ruida controllers (FTDI).
pub const USB_VENDOR_ID: u16 = 0x0403;
pub const USB_PRODUCT_ID: u16 = 0x6001;

/// The UDP checksum: sum of the scrambled payload bytes, mod 65536.
pub fn checksum(scrambled: &[u8]) -> u16 {
    scrambled.iter().fold(0u16, |acc, &b| acc.wrapping_add(u16::from(b)))
}

/// One UDP datagram from an unscrambled payload of whole commands.
pub fn frame_udp(swz: &Swizzle, plain: &[u8]) -> Result<Vec<u8>> {
    if plain.is_empty() || plain.len() > MAX_UDP_PAYLOAD {
        return Err(RuidaError::PayloadSize { len: plain.len(), max: MAX_UDP_PAYLOAD });
    }
    let body = swz.scramble(plain);
    let mut out = Vec::with_capacity(body.len() + 2);
    out.extend(checksum(&body).to_be_bytes());
    out.extend(body);
    Ok(out)
}

/// The unscrambled payload of a UDP datagram, after checking its checksum.
pub fn unframe_udp(swz: &Swizzle, packet: &[u8]) -> Result<Vec<u8>> {
    if packet.len() < 3 {
        return Err(RuidaError::PayloadSize { len: packet.len().saturating_sub(2), max: MAX_UDP_PAYLOAD });
    }
    let (head, body) = packet.split_at(2);
    let expected = u16::from_be_bytes([head[0], head[1]]);
    let got = checksum(body);
    if expected != got {
        return Err(RuidaError::Checksum { expected, got });
    }
    Ok(swz.unscramble(body))
}

/// USB serial: the scrambled bytes, nothing else.
pub fn frame_serial(swz: &Swizzle, plain: &[u8]) -> Vec<u8> {
    swz.scramble(plain)
}

/// Group an unscrambled command stream into datagram payloads of at most `max` bytes, never splitting a command.
pub fn packetize(plain: &[u8], max: usize) -> Result<Vec<Vec<u8>>> {
    let max = max.min(MAX_UDP_PAYLOAD);
    let mut out: Vec<Vec<u8>> = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    for (_, c) in split(plain) {
        if c.len() > max {
            return Err(RuidaError::PayloadSize { len: c.len(), max });
        }
        if cur.len() + c.len() > max {
            out.push(std::mem::take(&mut cur));
        }
        cur.extend_from_slice(c);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// Encode commands and group them into datagram payloads.
pub fn packetize_commands(cmds: &[Command], max: usize) -> Result<Vec<Vec<u8>>> {
    packetize(&crate::command::encode_all(cmds)?, max)
}

/// What the controller sends back (unscrambled).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Reply {
    /// `CC`
    Ack,
    /// `CF`
    Nak,
    /// `CD`
    Err,
    /// `CE`
    Enq,
    /// `DA 01 addr value [value]`: a memory read. MeerK40t's emulator sends one value; its client reads the first.
    Memory { addr: MemAddr, value: u64, second: Option<u64> },
    /// `E8 01 n name 00`: a stored document's name.
    DocumentName { index: u16, name: String },
    /// Anything else, unscrambled.
    Other(Vec<u8>),
}

/// What to do with a reply while waiting for a datagram's acknowledgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckAction {
    /// Taken: send the next datagram (or wait for the data reply).
    Proceed,
    /// Refused: send the same datagram again.
    Resend,
    /// Not an answer to the datagram: keep waiting.
    KeepWaiting,
}

impl Reply {
    pub fn decode(plain: &[u8]) -> Reply {
        match plain {
            [0xCC] => Reply::Ack,
            [0xCF] => Reply::Nak,
            [0xCD] => Reply::Err,
            [0xCE] => Reply::Enq,
            [0xDA, 0x01, a0, a1, rest @ ..] if rest.len() == 5 || rest.len() == 10 => {
                let addr = MemAddr::decode([*a0, *a1]);
                let v0 = decode_u35([rest[0], rest[1], rest[2], rest[3], rest[4]]);
                let v1 = if rest.len() == 10 {
                    decode_u35([rest[5], rest[6], rest[7], rest[8], rest[9]]).map(Some)
                } else {
                    Ok(None)
                };
                match (addr, v0, v1) {
                    (Ok(addr), Ok(value), Ok(second)) => Reply::Memory { addr, value, second },
                    _ => Reply::Other(plain.to_vec()),
                }
            }
            [0xE8, 0x01, n0, n1, name @ .., 0] if name.iter().all(|&b| b != 0 && b < 0x80) => {
                match decode_u14([*n0, *n1]) {
                    Ok(index) => Reply::DocumentName { index, name: String::from_utf8_lossy(name).into_owned() },
                    Err(_) => Reply::Other(plain.to_vec()),
                }
            }
            _ => Reply::Other(plain.to_vec()),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(match self {
            Reply::Ack => vec![0xCC],
            Reply::Nak => vec![0xCF],
            Reply::Err => vec![0xCD],
            Reply::Enq => vec![0xCE],
            Reply::Memory { addr, value, second } => {
                let mut v = vec![0xDA, 0x01];
                v.extend(addr.encode());
                v.extend(encode_u35(*value)?);
                if let Some(s) = second {
                    v.extend(encode_u35(*s)?);
                }
                v
            }
            Reply::DocumentName { index, name } => {
                if name.bytes().any(|b| b == 0 || b >= 0x80) {
                    return Err(RuidaError::BadName(name.clone()));
                }
                let mut v = vec![0xE8, 0x01];
                v.extend(encode_u14(*index)?);
                v.extend_from_slice(name.as_bytes());
                v.push(0);
                v
            }
            Reply::Other(b) => b.clone(),
        })
    }

    /// Decode a datagram (or serial bytes) from the controller.
    pub fn from_wire(swz: &Swizzle, bytes: &[u8]) -> Reply {
        Reply::decode(&swz.unscramble(bytes))
    }

    pub fn to_wire(&self, swz: &Swizzle) -> Result<Vec<u8>> {
        Ok(swz.scramble(&self.encode()?))
    }

    pub fn ack_action(&self) -> AckAction {
        match self {
            Reply::Ack => AckAction::Proceed,
            Reply::Nak | Reply::Err => AckAction::Resend,
            _ => AckAction::KeepWaiting,
        }
    }
}

/// Whether the controller answers this payload with data after the ACK: memory reads (`DA 00`) and document-name
/// requests (`E8 01`) do (MeerK40t treats every `DA` except `DA 01` as replying).
pub fn expects_data_reply(plain: &[u8]) -> bool {
    split(plain).any(|(_, c)| matches!(c, [0xDA, s, ..] if *s != 0x01) || matches!(c, [0xE8, 0x01, ..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem;

    #[test]
    fn udp_frame_round_trip_and_checksum() {
        let swz = Swizzle::default();
        let plain = Command::GetMemory(mem::CARD_ID).encode().unwrap();
        let pkt = frame_udp(&swz, &plain).unwrap();
        // Scrambled DA 00 05 7E = D4 89 0D F7 (MeerK40t test vector); sum = 0x0261.
        assert_eq!(pkt, [0x02, 0x61, 0xD4, 0x89, 0x0D, 0xF7]);
        assert_eq!(unframe_udp(&swz, &pkt).unwrap(), plain);
        let mut bad = pkt.clone();
        bad[3] ^= 1;
        assert!(matches!(unframe_udp(&swz, &bad), Err(RuidaError::Checksum { .. })));
        assert!(unframe_udp(&swz, &[0, 0]).is_err());
        assert!(frame_udp(&swz, &[]).is_err());
        assert!(frame_udp(&swz, &vec![0xCE; MAX_UDP_PAYLOAD + 1]).is_err());
        // The checksum wraps at 16 bits.
        assert_eq!(checksum(&vec![0xFF; 300]), u16::try_from((300 * 255) % 65536).unwrap());
    }

    #[test]
    fn serial_has_no_checksum() {
        let swz = Swizzle::new(0x11);
        assert_eq!(frame_serial(&swz, &[0xCE]), swz.scramble(&[0xCE]));
    }

    #[test]
    fn packetize_keeps_commands_whole() {
        let cmds: Vec<Command> = (0..500).map(|i| Command::CutAbs { x: i, y: -i }).collect();
        let plain = crate::command::encode_all(&cmds).unwrap();
        let packets = packetize(&plain, MAX_UDP_PAYLOAD).unwrap();
        assert!(packets.iter().all(|p| p.len() <= MAX_UDP_PAYLOAD && p.len() % 11 == 0));
        assert_eq!(packets.concat(), plain);
        let mut back = Vec::new();
        for p in &packets {
            back.extend(crate::command::parse(p).unwrap());
        }
        assert_eq!(back, cmds);
        // A command longer than the limit is refused.
        assert!(packetize(&plain, 10).is_err());
        assert_eq!(packetize(&[], 100).unwrap(), Vec::<Vec<u8>>::new());
        assert_eq!(packetize_commands(&cmds[..2], 11).unwrap().len(), 2);
    }

    #[test]
    fn replies() {
        let swz = Swizzle::default();
        assert_eq!(Reply::from_wire(&swz, &[0xC6]), Reply::Ack);
        assert_eq!(Reply::from_wire(&swz, &[0x46]), Reply::Err);
        assert_eq!(Reply::from_wire(&swz, &[0x48]), Reply::Nak);
        assert_eq!(Reply::Ack.to_wire(&swz).unwrap(), [0xC6]);
        assert_eq!(Reply::Err.to_wire(&swz).unwrap(), [0x46]);
        assert_eq!(Reply::Ack.ack_action(), AckAction::Proceed);
        assert_eq!(Reply::Err.ack_action(), AckAction::Resend);
        assert_eq!(Reply::Nak.ack_action(), AckAction::Resend);
        assert_eq!(Reply::Enq.ack_action(), AckAction::KeepWaiting);
        for r in [
            Reply::Ack,
            Reply::Nak,
            Reply::Err,
            Reply::Enq,
            Reply::Memory { addr: mem::POSITION_X, value: 12_345, second: None },
            Reply::Memory { addr: mem::CARD_ID, value: mem::CARD_ID_RDC6442S, second: Some(1) },
            Reply::DocumentName { index: 2, name: "TEST".into() },
            Reply::Other(vec![0xDA, 0x05, 0, 0]),
        ] {
            assert_eq!(Reply::decode(&r.encode().unwrap()), r);
            assert_eq!(Reply::from_wire(&swz, &r.to_wire(&swz).unwrap()), r);
        }
        assert!(Reply::DocumentName { index: 1, name: "\u{e9}".into() }.encode().is_err());
        // Memory reply with a bad value byte is kept raw.
        assert!(matches!(Reply::decode(&[0xDA, 0x01, 0, 0, 0x80, 0, 0, 0, 0]), Reply::Other(_)));
    }

    #[test]
    fn data_reply_expectation() {
        assert!(expects_data_reply(&Command::GetMemory(mem::MACHINE_STATUS).encode().unwrap()));
        assert!(expects_data_reply(&[0xE8, 0x01, 0, 1]));
        assert!(!expects_data_reply(&[0xDA, 0x01, 0, 0x26, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert!(!expects_data_reply(&[0xCE]));
    }
}
