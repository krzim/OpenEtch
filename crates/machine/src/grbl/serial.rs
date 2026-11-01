//! [`SerialPortLink`]: a real serial port (feature `serial`), through the `serial2` crate (BSD-2-Clause OR
//! Apache-2.0; its dependencies `libc`, `cfg-if` and `windows-sys` are MIT/Apache). Chosen over `serialport`
//! (MPL-2.0) because it is permissive and needs no system library (no libudev).

use std::io::ErrorKind;
use std::time::Duration;

use super::SerialLink;
use crate::link::LinkError;

/// An open serial port (8N1, no flow control, at the profile's baud rate).
pub struct SerialPortLink {
    port: serial2::SerialPort,
    timeout: Duration,
}

impl std::fmt::Debug for SerialPortLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SerialPortLink").finish_non_exhaustive()
    }
}

fn io(e: std::io::Error) -> LinkError {
    match e.kind() {
        ErrorKind::NotFound => LinkError::NotFound(e.to_string()),
        ErrorKind::BrokenPipe | ErrorKind::NotConnected | ErrorKind::ConnectionAborted => LinkError::Disconnected,
        ErrorKind::TimedOut => LinkError::Timeout,
        _ => LinkError::Io(e.to_string()),
    }
}

impl SerialPortLink {
    /// Open `name` (`COM5`, `/dev/ttyUSB0`) at `baud`.
    pub fn open(name: &str, baud: u32) -> Result<Self, LinkError> {
        if name.trim().is_empty() {
            return Err(LinkError::NotFound("no serial port set in the device's profile".into()));
        }
        let mut port = serial2::SerialPort::open(name.trim(), baud).map_err(io)?;
        port.set_write_timeout(Duration::from_secs(2)).map_err(io)?;
        let timeout = Duration::from_millis(10);
        port.set_read_timeout(timeout).map_err(io)?;
        Ok(SerialPortLink { port, timeout })
    }
}

impl SerialLink for SerialPortLink {
    fn write(&mut self, bytes: &[u8]) -> Result<(), LinkError> {
        self.port.write_all(bytes).map_err(io)?;
        self.port.flush().map_err(io)
    }

    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, LinkError> {
        let timeout = timeout.max(Duration::from_millis(1));
        if timeout != self.timeout {
            self.port.set_read_timeout(timeout).map_err(io)?;
            self.timeout = timeout;
        }
        match self.port.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted) => Ok(0),
            Err(e) => Err(io(e)),
        }
    }
}

/// The serial ports this computer has (names to put in a profile's `port`).
pub fn available_ports() -> Vec<String> {
    serial2::SerialPort::available_ports()
        .map(|v| v.into_iter().map(|p| p.to_string_lossy().into_owned()).collect())
        .unwrap_or_default()
}
