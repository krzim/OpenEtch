//! Memory registers: read with [`crate::Command::GetMemory`] (`DA 00 addr`), written with
//! [`crate::Command::SetMemory`] (`DA 01 addr v0 v1`). The controller answers a read with `DA 01 addr value`
//! ([`crate::frame::Reply::Memory`]).
//!
//! An address is a 14-bit number sent as two 7-bit bytes: the card ID register `0x02FE` goes out as `05 7E`, the X
//! position `0x0221` as `04 21`.
//!
//! The named registers come from MeerK40t (`rdjob.py` `MEM_*`, the reply decoder, and the emulator's `mem_lookup`).
//! Only the status, card ID and position registers were seen on a real controller there (an RDC6442S); none of them
//! has been checked against hardware here.

// Facts from meerk40t/meerk40t @ d511944 `meerk40t/ruida/rdjob.py` (`MEM_*`, `MACHINE_STATUS_*`, `CID_LUT`) and
// `meerk40t/ruida/emulator.py` (`mem_lookup`) (MIT License, Copyright (c) 2021 meerk40t; full notice in `swizzle.rs`).

use serde::{Deserialize, Serialize};

use crate::num::{decode_u14, encode_u14};
use crate::{Result, RuidaError};

/// A 14-bit memory register address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MemAddr(u16);

impl MemAddr {
    pub const fn new_const(addr: u16) -> Self {
        assert!(addr <= 0x3FFF, "memory addresses are 14-bit");
        MemAddr(addr)
    }

    pub fn new(addr: u16) -> Result<Self> {
        if addr > 0x3FFF {
            return Err(RuidaError::OutOfRange { what: "memory address", value: i64::from(addr), min: 0, max: 0x3FFF });
        }
        Ok(MemAddr(addr))
    }

    pub fn get(self) -> u16 {
        self.0
    }

    pub fn encode(self) -> [u8; 2] {
        // Always in range by construction.
        encode_u14(self.0).unwrap_or([0, 0])
    }

    pub fn decode(b: [u8; 2]) -> Result<Self> {
        Ok(MemAddr(decode_u14(b)?))
    }

    /// MeerK40t's name for the register, when it has one.
    pub fn name(self) -> Option<&'static str> {
        REGISTERS.iter().find(|(a, _)| *a == self).map(|(_, n)| *n)
    }
}

/// G0 (travel) velocity, µm/s.
pub const G0_VELOCITY: MemAddr = MemAddr::new_const(0x0005);
/// Home velocity, µm/s.
pub const HOME_VELOCITY: MemAddr = MemAddr::new_const(0x000C);
/// Laser 1 PWM frequency.
pub const LASER1_FREQUENCY: MemAddr = MemAddr::new_const(0x0011);
/// Laser 1 minimum power.
pub const LASER1_MIN_POWER: MemAddr = MemAddr::new_const(0x0012);
/// Laser 1 maximum power.
pub const LASER1_MAX_POWER: MemAddr = MemAddr::new_const(0x0013);
/// Laser 2 PWM frequency.
pub const LASER2_FREQUENCY: MemAddr = MemAddr::new_const(0x0017);
/// Laser 2 minimum power.
pub const LASER2_MIN_POWER: MemAddr = MemAddr::new_const(0x0018);
/// Laser 2 maximum power.
pub const LASER2_MAX_POWER: MemAddr = MemAddr::new_const(0x0019);
/// X axis range (bed width), µm.
pub const BED_SIZE_X: MemAddr = MemAddr::new_const(0x0026);
/// Y axis range (bed height), µm.
pub const BED_SIZE_Y: MemAddr = MemAddr::new_const(0x0036);
/// Z axis range, µm.
pub const BED_SIZE_Z: MemAddr = MemAddr::new_const(0x0046);
/// U axis range, µm.
pub const BED_SIZE_U: MemAddr = MemAddr::new_const(0x0056);
/// Machine status bits (see [`STATUS_JOB_RUNNING`] etc.).
pub const MACHINE_STATUS: MemAddr = MemAddr::new_const(0x0200);
/// Total number of documents (stored jobs) on the controller.
pub const TOTAL_DOC_NUMBER: MemAddr = MemAddr::new_const(0x0205);
/// Current X position, µm.
pub const POSITION_X: MemAddr = MemAddr::new_const(0x0221);
/// Current Y position, µm.
pub const POSITION_Y: MemAddr = MemAddr::new_const(0x0231);
/// Current Z position, µm.
pub const POSITION_Z: MemAddr = MemAddr::new_const(0x0241);
/// Current U position, µm.
pub const POSITION_U: MemAddr = MemAddr::new_const(0x0251);
/// Card (controller model) ID.
pub const CARD_ID: MemAddr = MemAddr::new_const(0x02FE);
/// Mainboard version (the reply is a NUL-terminated string in MeerK40t's emulator).
pub const MAINBOARD_VERSION: MemAddr = MemAddr::new_const(0x02FF);
/// Total length of the file being processed.
pub const FILE_TOTAL_LENGTH: MemAddr = MemAddr::new_const(0x0320);
/// Progress through the file being processed.
pub const FILE_PROGRESS: MemAddr = MemAddr::new_const(0x0321);

/// Status bit: a job is running (MeerK40t, RDC6442S).
pub const STATUS_JOB_RUNNING: u64 = 0x0000_0001;
/// Status bit: part end.
pub const STATUS_PART_END: u64 = 0x0000_0002;
/// Status bit: the head is moving.
pub const STATUS_MOVING: u64 = 0x0100_0000;

/// Card ID of the RDC6442S (MeerK40t `CID_LUT`).
pub const CARD_ID_RDC6442S: u64 = 0x6510_6510;

const REGISTERS: [(MemAddr, &str); 22] = [
    (G0_VELOCITY, "G0 velocity"),
    (HOME_VELOCITY, "home velocity"),
    (LASER1_FREQUENCY, "laser 1 PWM frequency"),
    (LASER1_MIN_POWER, "laser 1 min power"),
    (LASER1_MAX_POWER, "laser 1 max power"),
    (LASER2_FREQUENCY, "laser 2 PWM frequency"),
    (LASER2_MIN_POWER, "laser 2 min power"),
    (LASER2_MAX_POWER, "laser 2 max power"),
    (BED_SIZE_X, "X range (bed width)"),
    (BED_SIZE_Y, "Y range (bed height)"),
    (BED_SIZE_Z, "Z range"),
    (BED_SIZE_U, "U range"),
    (MACHINE_STATUS, "machine status"),
    (TOTAL_DOC_NUMBER, "total document number"),
    (POSITION_X, "X position"),
    (POSITION_Y, "Y position"),
    (POSITION_Z, "Z position"),
    (POSITION_U, "U position"),
    (CARD_ID, "card ID"),
    (MAINBOARD_VERSION, "mainboard version"),
    (FILE_TOTAL_LENGTH, "file total length"),
    (FILE_PROGRESS, "file progress"),
];

/// A position register's value (µm): the low 32 bits taken as signed, like absolute coordinates.
pub fn position_um(value: u64) -> i32 {
    u32::try_from(value & 0xFFFF_FFFF).map_or(0, u32::cast_signed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_bytes_of_known_registers() {
        // rdjob.py writes these as raw byte pairs.
        assert_eq!(CARD_ID.encode(), [0x05, 0x7E]);
        assert_eq!(MACHINE_STATUS.encode(), [0x04, 0x00]);
        assert_eq!(POSITION_X.encode(), [0x04, 0x21]);
        assert_eq!(POSITION_Y.encode(), [0x04, 0x31]);
        assert_eq!(POSITION_Z.encode(), [0x04, 0x41]);
        assert_eq!(POSITION_U.encode(), [0x04, 0x51]);
        assert_eq!(BED_SIZE_X.encode(), [0x00, 0x26]);
        assert_eq!(BED_SIZE_Y.encode(), [0x00, 0x36]);
        for (a, _) in REGISTERS {
            assert_eq!(MemAddr::decode(a.encode()).unwrap(), a);
            assert!(a.name().is_some());
        }
        assert!(MemAddr::new(0x4000).is_err());
        assert_eq!(MemAddr::new(0x0001).unwrap().name(), None);
    }

    #[test]
    fn positions() {
        assert_eq!(position_um(5), 5);
        assert_eq!(position_um(0x7_FFFF_FFFB), -5);
        assert_eq!(position_um(0xFFFF_FFFB), -5);
    }
}
