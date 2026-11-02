//! [`SimLink`]: a fake board that records every single command and packet. Used by tests and by the engine's
//! simulator mode. Never touches hardware.
//!
//! The simulated board answers every single (except `WriteCorLine`) with an 8-byte reply whose word 3 is the
//! status: `READY` unless a packet was just queued ([`SimBoard::not_ready_polls`]), plus `BUSY` from `ExecuteList`
//! until [`SimBoard::busy_polls`] `GetVersion` polls have passed (or `StopExecute`). It also tracks the MO state, the
//! port word and the galvo position set by single commands, so tests can check what a sequence leaves behind.
//! `StopList` pauses an executing list (its BUSY polls stop counting down) until `RestartList`; `MoveAxisTo` reports
//! `AXIS` for [`SimBoard::axis_polls`] polls; `ReadPort` answers [`SimLink::set_inputs`] in word 1.

use lmc::{Packet, Single, StatusWord, op};

use crate::link::{Link, LinkError, Reply};

/// One recorded transfer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Traffic {
    Single(Single),
    Packet(Packet),
}

/// How the simulated board behaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimBoard {
    /// Words 0..3 of the `GetSerialNo` reply.
    pub serial: [u16; 3],
    /// Words 0..3 of the `GetVersion` reply.
    pub version: [u16; 3],
    /// `GetVersion` polls the board reports BUSY after `ExecuteList`.
    pub busy_polls: usize,
    /// `GetVersion` polls the board reports not READY after each list packet.
    pub not_ready_polls: usize,
    /// When set: the not-READY polls after a packet sent while a list executes (a full buffer that frees up only as
    /// the board marks, as with slow, dense fills); `not_ready_polls` then applies only before execution starts.
    pub executing_not_ready_polls: Option<usize>,
    /// `GetVersion` polls the board reports AXIS (moving) after `MoveAxisTo`.
    pub axis_polls: usize,
}

impl Default for SimBoard {
    fn default() -> Self {
        SimBoard {
            serial: [0x5153, 0x494d, 0x0001],
            version: [0x0001, 0x0000, 0x0000],
            busy_polls: 3,
            not_ready_polls: 1,
            executing_not_ready_polls: None,
            axis_polls: 2,
        }
    }
}

/// A recording, simulated board. See the module docs.
#[derive(Debug, Clone, Default)]
pub struct SimLink {
    board: SimBoard,
    traffic: Vec<Traffic>,
    executing: bool,
    busy_left: usize,
    not_ready_left: usize,
    mo_open: bool,
    port: u16,
    position: (u16, u16),
    disconnected: bool,
    /// Fail the packet with this index (counted from 0 over the link's life) once, with this error.
    packet_failure: Option<(usize, LinkError)>,
    packets_sent: usize,
    paused: bool,
    axis_left: usize,
    axis_position: u16,
    inputs: u16,
}

impl SimLink {
    /// A board with [`SimBoard::default`] behaviour.
    pub fn new() -> Self {
        Self::with_board(SimBoard::default())
    }

    /// A board with the given behaviour.
    pub fn with_board(board: SimBoard) -> Self {
        SimLink { board, position: (lmc::CENTER, lmc::CENTER), ..Default::default() }
    }

    /// Everything sent so far, in order.
    pub fn traffic(&self) -> &[Traffic] {
        &self.traffic
    }

    /// Forget the recorded traffic (board state is kept).
    pub fn clear_traffic(&mut self) {
        self.traffic.clear();
    }

    /// The single commands sent so far, in order.
    pub fn singles(&self) -> Vec<Single> {
        self.traffic.iter().filter_map(|t| if let Traffic::Single(s) = t { Some(*s) } else { None }).collect()
    }

    /// The single commands without the status polls (`GetVersion`) and `SetEndOfList` markers: the sequence a
    /// human reads.
    pub fn singles_without_polls(&self) -> Vec<Single> {
        self.singles().into_iter().filter(|s| !matches!(s.opcode(), op::GET_VERSION | op::SET_END_OF_LIST)).collect()
    }

    /// The list packets sent so far, in order.
    pub fn packets(&self) -> Vec<Packet> {
        self.traffic.iter().filter_map(|t| if let Traffic::Packet(p) = t { Some(p.clone()) } else { None }).collect()
    }

    /// Whether the last `Fiber_SetMo` opened the master oscillator.
    pub fn mo_open(&self) -> bool {
        self.mo_open
    }

    /// The last port word written by a single `WritePort`.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The last `GotoXY` target (centre at start).
    pub fn position(&self) -> (u16, u16) {
        self.position
    }

    /// Whether a list is executing.
    pub fn executing(&self) -> bool {
        self.executing
    }

    /// Whether `StopList` paused the executing list (until `RestartList`).
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// The last `MoveAxisTo` position (0 at start).
    pub fn axis_position(&self) -> u16 {
        self.axis_position
    }

    /// Set the input port word `ReadPort` reports (word 1).
    pub fn set_inputs(&mut self, bits: u16) {
        self.inputs = bits;
    }

    /// From now on every transfer fails with [`LinkError::Disconnected`].
    pub fn disconnect(&mut self) {
        self.disconnected = true;
    }

    /// Make the `index`-th packet (counted over the link's life, from 0) fail once with `error`.
    pub fn fail_packet(&mut self, index: usize, error: LinkError) {
        self.packet_failure = Some((index, error));
    }

    fn status(&self) -> u16 {
        let mut s = 0;
        if self.not_ready_left == 0 {
            s |= StatusWord::READY;
        }
        if self.executing {
            s |= StatusWord::BUSY;
        }
        if self.axis_left > 0 {
            s |= StatusWord::AXIS;
        }
        s
    }

    /// One status poll passes: report the current status, then advance the counters.
    fn poll(&mut self) -> u16 {
        let s = self.status();
        self.not_ready_left = self.not_ready_left.saturating_sub(1);
        self.axis_left = self.axis_left.saturating_sub(1);
        if self.executing && !self.paused {
            self.busy_left = self.busy_left.saturating_sub(1);
            self.executing = self.busy_left > 0;
        }
        s
    }
}

impl Link for SimLink {
    fn send_single(&mut self, cmd: Single) -> Result<Option<Reply>, LinkError> {
        if self.disconnected {
            return Err(LinkError::Disconnected);
        }
        self.traffic.push(Traffic::Single(cmd));
        let [opcode, a, b, ..] = cmd.0;
        let mut words = [0u16; 3];
        match opcode {
            op::GET_VERSION => {
                words = self.board.version;
                let status = self.poll();
                return Ok(Some([words[0], words[1], words[2], status]));
            }
            op::GET_SERIAL_NO => words = self.board.serial,
            op::EXECUTE_LIST => {
                self.busy_left = self.board.busy_polls;
                self.executing = self.busy_left > 0;
            }
            op::STOP_EXECUTE => {
                self.executing = false;
                self.busy_left = 0;
                self.paused = false;
            }
            op::STOP_LIST => self.paused = self.executing,
            op::RESTART_LIST => self.paused = false,
            op::MOVE_AXIS_TO => {
                self.axis_position = a;
                self.axis_left = self.board.axis_polls;
            }
            op::GET_AXIS_POS => words[0] = self.axis_position,
            op::READ_PORT => words[0] = 0,
            op::RESET_LIST => self.not_ready_left = 0,
            op::FIBER_SET_MO => self.mo_open = a != 0,
            op::WRITE_PORT => self.port = a,
            op::GOTO_XY => self.position = (a, b),
            _ => {}
        }
        if !cmd.reads_reply() {
            return Ok(None);
        }
        if opcode == op::READ_PORT {
            words[1] = self.inputs;
        }
        Ok(Some([words[0], words[1], words[2], self.status()]))
    }

    fn send_packet(&mut self, packet: &Packet) -> Result<(), LinkError> {
        if self.disconnected {
            return Err(LinkError::Disconnected);
        }
        let index = self.packets_sent;
        self.packets_sent += 1;
        if let Some((at, _)) = &self.packet_failure
            && *at == index
            && let Some((_, e)) = self.packet_failure.take()
        {
            return Err(e);
        }
        self.traffic.push(Traffic::Packet(packet.clone()));
        self.not_ready_left = self.board.executing_not_ready_polls.filter(|_| self.executing).unwrap_or(self.board.not_ready_polls);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(link: &mut SimLink) -> StatusWord {
        StatusWord::from_reply(link.send_single(Single::get_version()).unwrap().unwrap())
    }

    #[test]
    fn replies_and_recording() {
        let mut l = SimLink::new();
        let serial = l.send_single(Single::get_serial_no()).unwrap().unwrap();
        assert_eq!(&serial[..3], &SimBoard::default().serial);
        assert_eq!(l.send_single(Single::write_cor_line(1, 2, true)).unwrap(), None);
        assert!(status(&mut l).idle());
        assert_eq!(l.singles().len(), 3);
    }

    #[test]
    fn busy_after_execute_then_idle() {
        let mut l = SimLink::with_board(SimBoard { busy_polls: 2, not_ready_polls: 1, ..SimBoard::default() });
        l.send_packet(&Packet::empty()).unwrap();
        assert!(!status(&mut l).ready());
        assert!(status(&mut l).ready());
        l.send_single(Single::execute_list()).unwrap();
        assert!(status(&mut l).busy());
        assert!(status(&mut l).busy());
        assert!(status(&mut l).idle());
        assert_eq!(l.packets().len(), 1);
    }

    #[test]
    fn tracks_mo_port_position_and_failures() {
        let mut l = SimLink::new();
        l.send_single(Single::fiber_set_mo(true)).unwrap();
        l.send_single(Single::write_port(0x0101)).unwrap();
        l.send_single(Single::goto_xy(1, 2, 0)).unwrap();
        assert!(l.mo_open());
        assert_eq!((l.port(), l.position()), (0x0101, (1, 2)));
        l.fail_packet(0, LinkError::Timeout);
        assert_eq!(l.send_packet(&Packet::empty()), Err(LinkError::Timeout));
        assert!(l.send_packet(&Packet::empty()).is_ok());
        l.disconnect();
        assert_eq!(l.send_single(Single::get_version()), Err(LinkError::Disconnected));
    }

    #[test]
    fn pause_axis_and_inputs() {
        let mut l = SimLink::with_board(SimBoard { busy_polls: 2, not_ready_polls: 0, ..SimBoard::default() });
        l.send_single(Single::execute_list()).unwrap();
        l.send_single(Single::stop_list()).unwrap();
        assert!(l.paused());
        for _ in 0..5 {
            assert!(status(&mut l).busy(), "a paused list stays busy");
        }
        l.send_single(Single::restart_list()).unwrap();
        assert!(status(&mut l).busy());
        assert!(status(&mut l).busy());
        assert!(status(&mut l).idle(), "the busy polls count down again once restarted");
        l.send_single(Single::move_axis_to(1234, false)).unwrap();
        assert!(status(&mut l).axis());
        assert!(status(&mut l).axis());
        assert!(!status(&mut l).axis());
        assert_eq!(l.axis_position(), 1234);
        l.set_inputs(1 << 14);
        assert_eq!(l.send_single(Single::read_port()).unwrap().unwrap()[1], 1 << 14);
    }
}
