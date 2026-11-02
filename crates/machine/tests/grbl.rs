//! The GRBL driver against the GRBL simulator: flow control, runs, framing, the console, Stop with a full buffer,
//! pause, alarms, the door, settings checks and the machine-driver interface. Unverified on hardware (no gantry).

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use machine::grbl::{
    FireKind, GcodeProgram, GrblController, GrblError, GrblPoll, GrblProfile, RX_BUFFER, SimGrbl, SimGrblConfig, SimGrblHandle,
};
use machine::{
    Armed, Controller, ControllerError, ControllerState, Driver, DriverJob, FrameStyle, InitSettings, Link, LinkError,
    MachineDriver, MachineProfile, PollConfig, Reply, RunControl, RunPart, SimLink, Traffic,
};

fn profile() -> GrblProfile {
    GrblProfile { work_area_mm: [300.0, 200.0], ..GrblProfile::default() }
}

fn connected(cfg: &SimGrblConfig, p: &GrblProfile) -> (GrblController<SimGrbl>, SimGrblHandle) {
    let sim = SimGrbl::with_config(cfg);
    let h = sim.handle();
    let mut c = GrblController::with_handles(sim, GrblPoll::immediate(), Default::default(), RunControl::default());
    c.init(p).expect("init");
    (c, h)
}

/// A square burned at 40 % (S400 of $30=1000), `n` times round.
fn square(n: usize) -> GcodeProgram {
    let mut l = vec!["G90 G21".to_owned(), "M4 S0".to_owned()];
    for _ in 0..n {
        l.extend(
            ["G0 X10 Y10", "G1 X60 Y10 S400 F3000", "G1 X60 Y60", "G1 X10 Y60", "G1 X10 Y10"].iter().map(|s| (*s).to_owned()),
        );
    }
    l.push("M5".into());
    GcodeProgram::new(l)
}

fn armed() -> Armed {
    Armed::approved_in_ui("r1".into())
}

#[test]
fn init_reads_the_board_and_fires_nothing() {
    let (c, h) = connected(&SimGrblConfig::default(), &profile());
    let snap = c.snapshot();
    assert_eq!((snap.version.as_deref(), snap.laser_mode, snap.s_max, snap.spindle_off), (Some("1.1h"), Some(true), Some(1000.0), true));
    assert_eq!(c.state(), ControllerState::Idle);
    let r = h.report();
    assert!(r.fires.is_empty() && r.blocks_executed == 0, "{r:?}");
    assert!(r.lines.iter().all(|l| l.starts_with('$')), "only queries at init: {:?}", r.lines);
}

#[test]
fn a_run_streams_within_the_buffer_and_fires_only_its_cuts() {
    let (mut c, h) = connected(&SimGrblConfig::default(), &profile());
    let prog = square(30);
    let report = c.run_program(&prog, armed()).unwrap();
    assert_eq!(report.packets, prog.len());
    let r = h.report();
    assert!(!r.overflowed && r.max_rx_bytes <= RX_BUFFER, "max {} bytes", r.max_rx_bytes);
    assert!(r.max_rx_bytes > 60, "the buffer was used ({} bytes)", r.max_rx_bytes);
    assert_eq!(r.cuts(), 4 * 30, "every G1 of the square, nothing else");
    assert!(r.fires.iter().all(|f| f.kind == FireKind::Cut && f.s == 400.0));
    assert_eq!(r.lines_received, 3 + 2 + prog.len(), "init's $$ $N $G, the run's $$ $N, then the program");
    assert_eq!((r.state.as_str(), r.position), ("Idle", [10.0, 10.0]));
    assert_eq!(c.state(), ControllerState::Idle);
    let p = c.run_control().progress();
    assert_eq!((p.packets_sent, p.packets_total), (prog.len(), prog.len()));
}

#[test]
fn framing_never_fires() {
    let (mut c, h) = connected(&SimGrblConfig::default(), &profile());
    let outline = vec![vec![(-20.0, -20.0), (20.0, -20.0), (20.0, 20.0), (-20.0, 20.0), (-20.0, -20.0)]];
    let style = FrameStyle { pointer: lmc::PointerBits::RED, blanking: true, speed_mm_s: Some(50.0) };
    let n = c.frame_styled(&outline, 20, &style).unwrap();
    assert_eq!(n, 1 + 5 * 20);
    assert_eq!(c.state(), ControllerState::Framing);
    while !c.poll_idle().unwrap() {}
    assert_eq!(c.state(), ControllerState::Idle);
    let r = h.report();
    assert!(r.fires.is_empty(), "{:?}", r.fires);
    assert!(!r.overflowed);
    let frame_lines: Vec<&String> = r.lines.iter().skip_while(|l| l.as_str() != "M5").collect();
    assert!(frame_lines.iter().skip(1).all(|l| l.starts_with("$J=G90G21X") && l.ends_with("F3000")), "{frame_lines:?}");
    assert!(frame_lines.iter().all(|l| !l.contains('S') && !l.contains("M3") && !l.contains("M4")));
    // Corner (-20, -20) of a 300 x 200 bed from the front-left origin.
    assert!(r.lines.iter().any(|l| l == "$J=G90G21X130Y80F3000"), "{:?}", r.lines);
    // Outside the bed or above the feed limit: refused before anything is sent.
    let before = h.report().lines_received;
    assert!(c.frame_styled(&[vec![(151.0, 0.0)]], 1, &style).is_err());
    assert!(c.frame_styled(&outline, 1, &FrameStyle { speed_mm_s: Some(1000.0), ..style }).is_err());
    assert_eq!(h.report().lines_received, before);
    // Even with a spindle left on by someone else, framing first turns it off.
    let fires = h.report().fires.len();
    c.frame_styled(&outline, 1, &style).unwrap();
    c.stop_frame().unwrap();
    assert_eq!(h.report().fires.len(), fires);
}

#[test]
fn stop_wins_with_a_full_buffer() {
    let (mut c, h) = connected(&SimGrblConfig::default(), &profile());
    let abort = c.abort_handle();
    let prog = square(200);
    // Let the board execute a few blocks, then stall it as on a long, slow move: the planner and the receive buffer
    // fill up and the run waits for room.
    h.run_blocks(0);
    let sim = h.clone();
    let runner = thread::spawn(move || {
        let r = c.run_program(&prog, armed());
        (r, c)
    });
    let start = Instant::now();
    loop {
        if sim.report().blocks_executed >= 12 {
            sim.stall(true);
        }
        let rep = sim.report();
        if rep.blocks_executed >= 12 && rep.planner_blocks == machine::grbl::PLANNER_BLOCKS && rep.rx_bytes > 100 {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "the run never filled the buffers: {rep:?}");
        thread::yield_now();
    }
    let at_stop = sim.report();
    assert!(at_stop.fires.iter().any(|f| f.kind == FireKind::Cut), "the laser was cutting");
    abort.request();
    let (result, mut c) = runner.join().unwrap();
    assert_eq!(result.unwrap_err(), ControllerError::Aborted);
    let r = sim.report();
    // After init's own soft reset.
    let rt: Vec<_> = r.realtime[1..].iter().collect();
    assert_eq!(rt.iter().map(|e| e.byte).collect::<Vec<_>>(), vec![b'!', 0x18], "feed hold then soft reset");
    // Both arrived while GRBL still held unparsed lines and a full planner: they were not queued behind them.
    assert!(rt[0].rx_bytes > 60 && rt[0].planner_blocks == machine::grbl::PLANNER_BLOCKS, "{rt:?}");
    // Nothing executed after the stop, nothing fired, and the laser is off after the reset.
    assert_eq!(r.blocks_executed, at_stop.blocks_executed);
    assert_eq!(r.fires.len(), at_stop.fires.len());
    assert!(!r.lit_now && r.resets == 2, "init's reset and the stop's");
    assert_eq!(r.state, "Alarm", "a reset while moving locks GRBL (alarm 3)");
    assert!(r.max_rx_bytes <= RX_BUFFER && !r.overflowed);
    // The stop is not gated: it can be sent again, and the next run is refused until a person unlocks.
    c.abort().unwrap();
    sim.stall(false);
    let e = c.run_program(&square(1), armed()).unwrap_err();
    assert!(matches!(e, ControllerError::Grbl(GrblError::Locked(Some(3)))), "{e}");
    c.unlock().unwrap();
    c.run_program(&square(1), armed()).unwrap();
}

#[test]
fn a_stop_requested_before_the_run_sends_nothing_that_can_fire() {
    let (mut c, h) = connected(&SimGrblConfig::default(), &profile());
    c.abort_handle().request();
    assert_eq!(c.run_program(&square(1), armed()).unwrap_err(), ControllerError::Aborted);
    let r = h.report();
    assert!(r.fires.is_empty() && !r.lines.iter().any(|l| l.starts_with('G') || l.starts_with('M')), "{:?}", r.lines);
}

#[test]
fn pause_is_feed_hold_and_resume_cycle_start() {
    let (mut c, h) = connected(&SimGrblConfig::default(), &profile());
    let control = c.run_control();
    control.request_pause();
    let runner = thread::spawn(move || {
        let r = c.run_program(&square(3), armed());
        (r, c)
    });
    let start = Instant::now();
    while !control.is_paused() {
        assert!(start.elapsed() < Duration::from_secs(3));
        thread::yield_now();
    }
    thread::sleep(Duration::from_millis(30));
    let held = h.report();
    assert_eq!(held.blocks_executed, 0, "held before the first move");
    control.request_resume();
    let (r, _) = runner.join().unwrap();
    r.unwrap();
    let bytes: Vec<u8> = h.report().realtime[1..].iter().map(|e| e.byte).collect();
    assert_eq!(bytes, vec![b'!', b'~']);
    assert_eq!(h.report().cuts(), 12);
}

#[test]
fn the_board_must_be_in_laser_mode_with_the_profiles_power_range() {
    let p = profile();
    let (mut c, h) = connected(&SimGrblConfig { laser_mode: false, ..SimGrblConfig::default() }, &p);
    let e = c.run_program(&square(1), armed()).unwrap_err();
    assert!(e.to_string().contains("laser mode is off"), "{e}");
    let (mut c2, h2) = connected(&SimGrblConfig { s_max: 255.0, ..SimGrblConfig::default() }, &p);
    let e = c2.run_program(&square(1), armed()).unwrap_err();
    assert!(e.to_string().contains("$30 is 255"), "{e}");
    // A setting changed behind OpenEtch's back after init is caught at the next run.
    let (mut c3, h3) = connected(&SimGrblConfig::default(), &p);
    h3.set_setting(32, 0.0);
    assert!(c3.run_program(&square(1), armed()).is_err());
    for r in [h.report(), h2.report(), h3.report()] {
        assert!(r.fires.is_empty() && !r.lines.iter().any(|l| l.starts_with("G1")), "{:?}", r.lines);
    }
}

#[test]
fn programs_outside_the_limits_are_refused_before_sending() {
    let (mut c, h) = connected(&SimGrblConfig::default(), &GrblProfile { max_power_pct: 30.0, ..profile() });
    let e = c.run_program(&square(1), armed()).unwrap_err();
    assert!(matches!(e, ControllerError::Grbl(GrblError::Program(_))), "{e}");
    assert_eq!(h.report().lines_received, 3, "only init's queries");
}

#[test]
fn an_open_door_refuses_and_aborts_runs() {
    let (mut c, h) = connected(&SimGrblConfig::default(), &profile());
    h.set_door(true);
    assert!(matches!(c.run_program(&square(1), armed()), Err(ControllerError::Grbl(GrblError::DoorOpen))));
    h.set_door(false);
    let abort_on_door = h.clone();
    let runner = thread::spawn(move || c.run_program(&square(100), armed()));
    let start = Instant::now();
    while abort_on_door.report().blocks_executed < 5 {
        assert!(start.elapsed() < Duration::from_secs(3));
        thread::yield_now();
    }
    abort_on_door.set_door(true);
    assert!(matches!(runner.join().unwrap(), Err(ControllerError::Grbl(GrblError::DoorOpen))));
    let r = h.report();
    assert!(r.resets >= 1 && !r.lit_now);
}

#[test]
fn homing_unlocks_and_alarms_lock() {
    let p = GrblProfile { homing: true, ..profile() };
    let (mut c, h) = connected(&SimGrblConfig { homing: true, ..SimGrblConfig::default() }, &p);
    assert_eq!(h.report().state, "Alarm", "a board with homing starts locked");
    let style = FrameStyle { pointer: lmc::PointerBits::RED, blanking: false, speed_mm_s: None };
    let e = c.frame_styled(&[vec![(0.0, 0.0)]], 1, &style).unwrap_err();
    assert!(matches!(e, ControllerError::Grbl(GrblError::Locked(_))), "{e}");
    assert!(c.jog(1.0, 0.0, None).is_err());
    c.home().unwrap();
    assert_eq!(h.report().state, "Idle");
    c.jog(10.0, -5.0, Some(1200.0)).unwrap();
    c.jog_to(0.0, 0.0, None).unwrap();
    while !c.poll_idle().unwrap() {}
    let r = h.report();
    assert_eq!(r.position, [150.0, 100.0], "jogged to the bed centre");
    assert!(r.fires.is_empty());
    assert!(r.lines.iter().any(|l| l == "$J=G91G21X10Y-5F1200"), "{:?}", r.lines);
    // Homing off in the profile: refused without sending.
    let (mut c2, _) = connected(&SimGrblConfig::default(), &profile());
    assert!(c2.home().is_err());
}

#[test]
fn an_unarmed_console_never_fires() {
    let (mut c, h) = connected(&SimGrblConfig { laser_mode: false, ..SimGrblConfig::default() }, &profile());
    // With laser mode off, M3 S1000 alone would light the laser at once: the policy refuses it before sending.
    for bad in ["M3 S1000", "M4 S10", "M3", "S500", "G1 X10 F100 S5", "$N0=M3 S1000", "$RST=$", "?", "$J=X5 F100 S100"] {
        let before = h.report().lines_received;
        let e = c.console(bad).unwrap_err();
        assert!(matches!(e, ControllerError::Grbl(GrblError::Console(_))), "{bad}: {e}");
        assert_eq!(h.report().lines_received, before, "{bad} was sent");
    }
    for good in ["$$", "$G", "G90 G21", "G0 X50 Y50", "G1 X60 Y50 F600", "G1 X60 Y60 S0", "M5", "M8", "M9", "$J=G91 X-5 F500", "$110=6000"] {
        let r = c.console(good).unwrap();
        assert!(r.ok, "{good}: {r:?}");
    }
    let r = c.console("$$").unwrap();
    assert!(r.lines.iter().any(|l| l == "$32=0"), "{r:?}");
    assert!(c.console("G5").is_err(), "not on the allowlist: never sent");
    h.reject_line("G0 X99", 33);
    let r = c.console("G0 X99").unwrap();
    assert!(!r.ok && r.error.unwrap().contains("error 33"));
    while !c.poll_idle().unwrap() {}
    let rep = h.report();
    assert!(rep.fires.is_empty() && !rep.lit_now, "{:?}", rep.fires);
    assert!(rep.blocks_executed >= 3, "the moves ran");
}

#[test]
fn start_up_blocks_that_can_fire_block_motion_until_cleared() {
    let cfg = SimGrblConfig { startup: ["M3 S1000".into(), String::new()], ..SimGrblConfig::default() };
    let (mut c, h) = connected(&cfg, &profile());
    assert_eq!(c.snapshot().unsafe_startup, vec!["$N0=M3 S1000".to_owned()]);
    assert!(c.run_program(&square(1), armed()).is_err());
    assert!(c.console("G0 X10").is_err());
    let style = FrameStyle { pointer: lmc::PointerBits::RED, blanking: false, speed_mm_s: None };
    assert!(c.frame_styled(&[vec![(0.0, 0.0)]], 1, &style).is_err());
    assert!(c.console("$N0=").unwrap().ok);
    assert!(c.snapshot().unsafe_startup.is_empty());
    // The init's soft reset ran the old block (M3 S1000, no motion, laser mode on: dark); clear the parser state.
    assert!(c.console("M5").unwrap().ok);
    c.run_program(&square(1), armed()).unwrap();
    assert_eq!(h.report().cuts(), 4);
}

#[test]
fn errors_from_the_board_abort_the_run() {
    let (mut c, h) = connected(&SimGrblConfig::default(), &profile());
    // The board refuses the 7th line (a move it cannot plan): the run stops with the code, nothing more fires.
    h.reject_line("X10 Y60", 33);
    let e = c.run_program(&square(50), armed()).unwrap_err();
    match &e {
        ControllerError::Grbl(GrblError::Rejected { code: 33, line }) => assert_eq!(line, "G1 X10 Y60"),
        other => panic!("{other}"),
    }
    assert!(e.to_string().contains("invalid motion target"), "{e}");
    let r = h.report();
    assert_eq!(r.resets, 2, "init's reset, then the stop sequence");
    assert!(r.cuts() <= 2 && !r.lit_now, "{:?}", r.fires);
    // A board that goes away mid-run: the error is the link's; the stop sequence is tried.
    let (mut c, h) = connected(&SimGrblConfig::default(), &profile());
    let sim = h.clone();
    let runner = thread::spawn(move || c.run_program(&square(100), armed()));
    let start = Instant::now();
    while sim.report().blocks_executed < 5 {
        assert!(start.elapsed() < Duration::from_secs(3));
        thread::yield_now();
    }
    sim.disconnect();
    let e = runner.join().unwrap().unwrap_err();
    assert!(matches!(e, ControllerError::Link(LinkError::Disconnected)), "{e}");
}

/// A link that records every transfer where a test can see it after the controller (boxed) owns it.
struct Recording(SimLink, Arc<Mutex<Vec<Traffic>>>);

impl Link for Recording {
    fn send_single(&mut self, cmd: lmc::Single) -> Result<Option<Reply>, LinkError> {
        self.1.lock().unwrap().push(Traffic::Single(cmd));
        self.0.send_single(cmd)
    }

    fn send_packet(&mut self, packet: &lmc::Packet) -> Result<(), LinkError> {
        self.1.lock().unwrap().push(Traffic::Packet(packet.clone()));
        self.0.send_packet(packet)
    }
}

fn light_and_mark_job() -> Vec<lmc::Packet> {
    let mut l = lmc::MarkingList::new();
    l.mark_power(lmc::units::power_to_mark_current(20.0).unwrap()).unwrap();
    l.jump_to(30000, 30000);
    for i in 0..700u16 {
        l.mark_to(30000 + i, 31000);
    }
    l.finish()
}

#[test]
fn the_lmc_driver_behind_the_interface_sends_the_same_bytes() {
    let profile = MachineProfile::generic_lmc_fiber();
    let packets = light_and_mark_job();
    let outline = vec![vec![(-5.0, -5.0), (5.0, -5.0), (5.0, 5.0)]];
    let style = FrameStyle::for_profile(&profile);

    let mut direct = Controller::with_settings(SimLink::new(), InitSettings::default(), PollConfig::immediate());
    direct.init(&profile, None).unwrap();
    direct.frame_styled(&outline, 2, &style).unwrap();
    direct.stop_frame().unwrap();
    direct.run_parts(&[RunPart::list(packets.clone())], armed()).unwrap();
    direct.abort().unwrap();

    let log = Arc::new(Mutex::new(Vec::new()));
    let link: Box<dyn Link> = Box::new(Recording(SimLink::new(), log.clone()));
    let mut d = Driver::Lmc(Controller::with_settings(link, InitSettings::default(), PollConfig::immediate()));
    d.lmc_mut().unwrap().init(&profile, None).unwrap();
    MachineDriver::frame_styled(&mut d, &outline, 2, &style).unwrap();
    MachineDriver::stop_frame(&mut d).unwrap();
    d.run_job(&DriverJob::Lmc(Arc::new(vec![RunPart::list(packets)])), armed()).unwrap();
    MachineDriver::abort(&mut d).unwrap();
    assert_eq!(*log.lock().unwrap(), direct.link().traffic(), "byte-identical on the wire");

    // A job made for the other driver is refused before anything is sent.
    let n = log.lock().unwrap().len();
    let e = d.run_job(&DriverJob::Grbl(Arc::new(square(1))), armed()).unwrap_err();
    assert!(e.to_string().contains("cannot run on a lmc machine"), "{e}");
    assert_eq!(log.lock().unwrap().len(), n);
    assert!(d.door_open().is_none());
}

#[test]
fn the_grbl_driver_behind_the_interface() {
    let sim = SimGrbl::new();
    let h = sim.handle();
    let port: Box<dyn machine::grbl::SerialLink> = Box::new(sim);
    let mut d = Driver::Grbl(GrblController::with_handles(port, GrblPoll::immediate(), Default::default(), RunControl::default()));
    d.grbl_mut().unwrap().init(&profile()).unwrap();
    assert!(matches!(d.read_port(), Err(ControllerError::Refused(_))));
    assert!(d.move_axis(10).is_err());
    assert_eq!(d.door_open().map(Result::unwrap), Some(false));
    let style = FrameStyle { pointer: lmc::PointerBits::RED, blanking: false, speed_mm_s: None };
    d.frame_styled(&[vec![(0.0, 0.0), (10.0, 0.0)]], 1, &style).unwrap();
    while !d.poll_idle().unwrap() {}
    assert!(d.run_job(&DriverJob::Lmc(Arc::new(vec![])), armed()).is_err());
    d.run_job(&DriverJob::Grbl(Arc::new(square(1))), armed()).unwrap();
    assert_eq!(h.report().cuts(), 4);
    d.abort().unwrap();
    assert_eq!(d.state(), ControllerState::Idle);
}

/// A start-up block that turned the spindle on (run by init's soft reset) and was then cleared leaves `M3 S1000` in
/// the parser state: nothing unarmed may move or dwell until the board reports `M5`.
#[test]
fn a_cleared_start_up_block_leaves_nothing_unarmed_moving_until_m5() {
    let cfg = SimGrblConfig { startup: ["M3 S1000".into(), String::new()], ..SimGrblConfig::default() };
    let (mut c, h) = connected(&cfg, &profile());
    assert!(c.console("$N0=").unwrap().ok);
    assert!(c.snapshot().unsafe_startup.is_empty() && !c.snapshot().spindle_off);
    let before = h.report().lines_received;
    for e in [c.jog(5.0, 0.0, None).unwrap_err(), c.jog_to(10.0, 10.0, None).unwrap_err()] {
        assert!(e.to_string().contains("not M5"), "{e}");
    }
    for line in ["G4 P1", "G0 X10", "$J=G91 X5 F500"] {
        let e = c.console(line).unwrap_err();
        assert!(e.to_string().contains("not M5"), "{line}: {e}");
    }
    let r = h.report();
    // Only the `$G` reads went out.
    assert!(r.lines[r.lines.len() - (r.lines_received - before)..].iter().all(|l| l == "$G"), "{:?}", r.lines);
    assert!(c.console("M5").unwrap().ok);
    c.jog(5.0, 0.0, None).unwrap();
    assert!(c.console("G4 P0.1").unwrap().ok);
    while !c.poll_idle().unwrap() {}
    let r = h.report();
    assert!(r.fires.is_empty() && !r.lit_now, "{:?}", r.fires);
}

/// A stop whose `!` and soft reset never reached the board (a flaky cable): the board finishes what it had buffered
/// and keeps `M4 S400`. The controller must not assume the reset happened: unarmed moves are refused until the
/// parser state says `M5`.
#[test]
fn a_stop_that_never_arrived_leaves_unarmed_moves_refused() {
    let (mut c, h) = connected(&SimGrblConfig::default(), &profile());
    let abort = c.abort_handle();
    let sim = h.clone();
    let runner = thread::spawn(move || {
        let r = c.run_program(&square(200), armed());
        (r, c)
    });
    let start = Instant::now();
    while sim.report().cuts() < 3 {
        assert!(start.elapsed() < Duration::from_secs(5));
        thread::yield_now();
    }
    sim.lose_stop_bytes(true);
    abort.request();
    let (result, mut c) = runner.join().unwrap();
    assert_eq!(result.unwrap_err(), ControllerError::Aborted);
    assert!(!c.snapshot().spindle_off, "the reset was never confirmed");
    // The board ran out its buffer and sits idle with the laser mode's spindle still on.
    while !c.poll_idle().unwrap() {}
    let fires = h.report().fires.len();
    for e in [c.jog(5.0, 0.0, None).unwrap_err(), c.console("G1 X20 Y20 F600").unwrap_err(), c.console("G4 P1").unwrap_err()] {
        assert!(e.to_string().contains("not M5"), "{e}");
    }
    while !c.poll_idle().unwrap() {}
    let r = h.report();
    assert_eq!(r.fires.len(), fires, "nothing unarmed fired: {:?}", &r.fires[fires..]);
    assert!(!r.lines.iter().any(|l| l.starts_with("$J=") || l == "G1 X20 Y20 F600"), "{:?}", r.lines);
    // A stop that does arrive resets the board, and moves work again.
    sim.lose_stop_bytes(false);
    c.abort().unwrap();
    assert!(c.snapshot().spindle_off);
    c.jog(5.0, 0.0, None).unwrap();
}

/// GRBL's check mode (`$C`) parses lines without moving or firing: a run there would report success with nothing
/// marked, so it is refused.
#[test]
fn runs_are_refused_in_check_mode() {
    let (mut c, h) = connected(&SimGrblConfig::default(), &profile());
    assert!(c.console("$C").unwrap().ok);
    let e = c.run_program(&square(1), armed()).unwrap_err();
    assert!(e.to_string().contains("check mode"), "{e}");
    assert!(!h.report().lines.iter().any(|l| l.starts_with("M4")));
    assert!(c.console("$C").unwrap().ok);
    c.run_program(&square(1), armed()).unwrap();
    assert_eq!(h.report().cuts(), 4);
}
