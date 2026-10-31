//! [`UsbLink`]: the real LMC board over nusb 0.2 (WinUSB on Windows). Feature `usb`. Never run by tests.
//!
//! Interface 0, bulk OUT `0x02` (12-byte singles, 3072-byte packets), bulk IN `0x88` (8-byte replies). Every
//! transfer has a timeout; a timed-out transfer is cancelled and its late completion skipped. No resets, no
//! retries, no kernel-driver detach: every failure is returned to the controller, which aborts.

// Adapted from openlaser-rs @ 9d81a15 `crates/transport/src/usb.rs` (MIT OR Apache-2.0, used here under MIT):
// endpoint layout, the 9588:9980 refusal, the WinUSB driver check, the location string, and the submit +
// `wait_next_complete` + cancel pattern. The bench/lock/keep-awake machinery is not ported.
//
// MIT License
//
// Copyright (c) 2026 Luiz Bueno and the OpenLaser contributors
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::time::{Duration, Instant};

use lmc::{PACKET_LEN, Packet, REPLY_LEN, Single};
use nusb::transfer::{Buffer, Bulk, Completion, In, Out, TransferError};
use nusb::{DeviceInfo, Endpoint, Interface, MaybeFuture};
use serde::Serialize;

use crate::link::{Link, LinkError, Reply};

/// BJJCZ vendor id.
pub const LMC_VID: u16 = 0x9588;
/// USBLMCV4, the LMC board this crate drives.
pub const LMC_PID: u16 = 0x9899;
/// The JCZ USB loader board: needs firmware loaded first, which this crate never does.
pub const CLONE_PID: u16 = 0x9980;
const EP_OUT: u8 = 0x02;
const EP_IN: u8 = 0x88;
const SINGLE_LEN: usize = 12;
/// Default per-transfer timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_millis(500);

/// One BJJCZ (`0x9588`) device found by [`detect`]. Descriptors only; the device is not opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetectedBoard {
    pub vid: u16,
    pub pid: u16,
    /// e.g. "bus 1 address 4".
    pub location: String,
    pub serial: Option<String>,
    /// Whether [`UsbLink::open`] can use it.
    pub supported: bool,
    pub note: String,
}

/// List BJJCZ devices (read-only enumeration; nothing is opened).
pub fn detect() -> Result<Vec<DetectedBoard>, LinkError> {
    let devices = nusb::list_devices().wait().map_err(|e| LinkError::Io(format!("USB enumeration: {e}")))?;
    Ok(devices.filter(|d| d.vendor_id() == LMC_VID).map(|d| board(&d)).collect())
}

fn board(d: &DeviceInfo) -> DetectedBoard {
    let pid = d.product_id();
    let (supported, note) = match pid {
        LMC_PID => winusb_check(d),
        CLONE_PID => (false, "JCZ USB Loader, needs firmware: unsupported".to_owned()),
        other => (false, format!("BJJCZ device 9588:{other:04x} is not the LMC board (9588:9899): unsupported")),
    };
    DetectedBoard { vid: d.vendor_id(), pid, location: location(d), serial: d.serial_number().map(str::to_owned), supported, note }
}

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
fn location(d: &DeviceInfo) -> String {
    format!("bus {} address {}", d.bus_id(), d.device_address())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn location(_d: &DeviceInfo) -> String {
    "unknown".to_owned()
}

/// On Windows nusb reaches the board only through WinUSB (Zadig: USBLMCV4 -> WinUSB).
#[cfg(target_os = "windows")]
fn winusb_check(d: &DeviceInfo) -> (bool, String) {
    match d.driver() {
        Some(drv) if drv.eq_ignore_ascii_case("winusb") => (true, "JCZ LMC (USBLMCV4) on WinUSB".to_owned()),
        other => (
            false,
            format!(
                "found, but not on WinUSB (driver: {}): switch it with Zadig (USBLMCV4, 9588:9899 -> WinUSB); EzCad needs its driver back afterwards",
                other.unwrap_or("none")
            ),
        ),
    }
}

#[cfg(not(target_os = "windows"))]
fn winusb_check(_d: &DeviceInfo) -> (bool, String) {
    (true, "JCZ LMC (USBLMCV4)".to_owned())
}

/// How to open the board.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsbOptions {
    /// Pick the board with this serial when several are connected.
    pub serial: Option<String>,
    /// Per-transfer timeout ([`DEFAULT_TIMEOUT`] when `None`).
    pub timeout: Option<Duration>,
    /// Select configuration 1 when the device is unconfigured (needed on some macOS setups). Off by default.
    pub set_configuration: bool,
}

/// The LMC board over USB. See the module docs.
pub struct UsbLink {
    out: Endpoint<Bulk, Out>,
    inp: Endpoint<Bulk, In>,
    _interface: Interface,
    timeout: Duration,
    /// Cancelled OUT/IN transfers whose completions are still to come; skipped when they arrive.
    stale_out: usize,
    stale_in: usize,
}

impl std::fmt::Debug for UsbLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UsbLink").field("timeout", &self.timeout).finish_non_exhaustive()
    }
}

fn map_transfer(e: TransferError) -> LinkError {
    match e {
        TransferError::Stall => LinkError::Stall,
        TransferError::Disconnected => LinkError::Disconnected,
        other => LinkError::Io(other.to_string()),
    }
}

fn io(what: &str, e: impl std::fmt::Display) -> LinkError {
    LinkError::Io(format!("{what}: {e}"))
}

/// Wait for the next completion that is not stale, until `deadline`.
fn next_completion<D: nusb::transfer::EndpointDirection>(
    ep: &mut Endpoint<Bulk, D>,
    stale: &mut usize,
    deadline: Instant,
) -> Option<Completion> {
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let c = ep.wait_next_complete(left)?;
        if *stale > 0 {
            *stale -= 1;
            continue;
        }
        return Some(c);
    }
}

impl UsbLink {
    /// Open the 9588:9899 board: claim interface 0, bulk OUT 0x02, bulk IN 0x88. Refuses the 9588:9980 loader board,
    /// a board not on WinUSB (Windows), several boards without a serial, and an unconfigured device unless
    /// [`UsbOptions::set_configuration`].
    pub fn open(options: &UsbOptions) -> Result<UsbLink, LinkError> {
        let all: Vec<DeviceInfo> = nusb::list_devices().wait().map_err(|e| io("USB enumeration", e))?.filter(|d| d.vendor_id() == LMC_VID).collect();
        let wanted: Vec<&DeviceInfo> = all
            .iter()
            .filter(|d| d.product_id() == LMC_PID)
            .filter(|d| options.serial.as_deref().is_none_or(|s| d.serial_number() == Some(s)))
            .collect();
        let info = match wanted.as_slice() {
            [one] => *one,
            [] if all.iter().any(|d| d.product_id() == CLONE_PID) => {
                return Err(LinkError::Unsupported("JCZ USB Loader (9588:9980), needs firmware: unsupported".into()));
            }
            [] => return Err(LinkError::NotFound("no JCZ LMC board (9588:9899) is connected".into())),
            many => return Err(LinkError::NotFound(format!("{} LMC boards are connected: choose one by serial", many.len()))),
        };
        let (usable, note) = winusb_check(info);
        if !usable {
            return Err(LinkError::Unsupported(note));
        }
        let device = info.open().wait().map_err(|e| io("open the board", e))?;
        if device.active_configuration().is_err() {
            if !options.set_configuration {
                return Err(LinkError::Unsupported("the board is unconfigured (enable set_configuration to select configuration 1)".into()));
            }
            device.set_configuration(1).wait().map_err(|e| io("set configuration 1", e))?;
        }
        let interface = device.claim_interface(0).wait().map_err(|e| io("claim interface 0", e))?;
        let out = interface.endpoint::<Bulk, Out>(EP_OUT).map_err(|e| io("bulk OUT 0x02", e))?;
        let inp = interface.endpoint::<Bulk, In>(EP_IN).map_err(|e| io("bulk IN 0x88", e))?;
        Ok(UsbLink { out, inp, _interface: interface, timeout: options.timeout.unwrap_or(DEFAULT_TIMEOUT), stale_out: 0, stale_in: 0 })
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), LinkError> {
        let mut buf = Buffer::new(bytes.len());
        buf.extend_from_slice(bytes);
        self.out.submit(buf);
        let deadline = Instant::now() + self.timeout;
        match next_completion(&mut self.out, &mut self.stale_out, deadline) {
            Some(c) => c.status.map_err(map_transfer),
            None => {
                self.out.cancel_all();
                self.stale_out = self.out.pending();
                Err(LinkError::Timeout)
            }
        }
    }

    fn read_reply(&mut self) -> Result<Reply, LinkError> {
        let n = self.inp.max_packet_size().max(REPLY_LEN);
        self.inp.submit(Buffer::new(n));
        let deadline = Instant::now() + self.timeout;
        let Some(c) = next_completion(&mut self.inp, &mut self.stale_in, deadline) else {
            self.inp.cancel_all();
            self.stale_in = self.inp.pending();
            return Err(LinkError::Timeout);
        };
        c.status.map_err(map_transfer)?;
        let data: &[u8] = &c.buffer;
        let reply = data.get(..REPLY_LEN).ok_or_else(|| LinkError::Io(format!("short reply: {} bytes", data.len())))?;
        lmc::parse_reply(reply).map_err(|e| LinkError::Io(e.to_string()))
    }
}

impl Link for UsbLink {
    fn send_single(&mut self, cmd: Single) -> Result<Option<Reply>, LinkError> {
        let bytes = cmd.to_bytes();
        debug_assert_eq!(bytes.len(), SINGLE_LEN);
        self.write(&bytes)?;
        if cmd.reads_reply() { self.read_reply().map(Some) } else { Ok(None) }
    }

    fn send_packet(&mut self, packet: &Packet) -> Result<(), LinkError> {
        let bytes = packet.as_bytes();
        debug_assert_eq!(bytes.len(), PACKET_LEN);
        self.write(bytes)
    }
}
