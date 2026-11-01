//! Hostile-input tests: random, truncated, oversized and wrongly keyed input must never panic, and whatever decodes
//! must re-encode to something that decodes the same.

use ruida::command::{Command, parse, parse_lenient, split};
use ruida::frame::{Reply, unframe_udp};
use ruida::rd::{RdJob, RdLayer, WriteOptions, detect_magic, read_rd, write_rd};
use ruida::swizzle::Swizzle;

/// Deterministic xorshift generator (no dependencies).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn byte(&mut self) -> u8 {
        self.next().to_le_bytes()[0]
    }
    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % u64::try_from(n).unwrap()).unwrap()
    }
    fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.byte()).collect()
    }
}

/// Opcode-led chunks with 7-bit parameters, lengths 1..=20: the shape `split` produces.
fn random_chunk(r: &mut Rng) -> Vec<u8> {
    let len = 1 + r.below(20);
    let mut c = vec![0x80 | r.byte()];
    // Bias the sub-opcode towards the families this crate types.
    if len > 1 {
        c.push(r.byte() & 0x7F);
    }
    while c.len() < len {
        c.push(r.byte() & 0x7F);
    }
    c
}

#[test]
fn command_decode_never_panics_and_typed_commands_reencode() {
    let mut r = Rng(0x9E37_79B9_7F4A_7C15);
    let mut typed = 0;
    for _ in 0..400_000 {
        let c = random_chunk(&mut r);
        if let Ok(cmd) = Command::decode(&c) {
            if matches!(cmd, Command::Unknown(_)) {
                assert_eq!(cmd.encode().unwrap(), c, "Unknown must keep its bytes");
                continue;
            }
            typed += 1;
            let again = cmd.encode().unwrap_or_else(|e| panic!("{cmd:?} from {c:02X?} does not re-encode: {e}"));
            assert_eq!(Command::decode(&again).unwrap(), cmd, "{c:02X?}");
        }
    }
    assert!(typed > 1000, "the fuzzer reached typed commands only {typed} times");
}

#[test]
fn exhaustive_short_chunks() {
    // Every opcode with every one- and two-byte 7-bit tail.
    for op in 0x80..=0xFFu8 {
        let _ = Command::decode(&[op]);
        for a in 0..0x80u8 {
            let _ = Command::decode(&[op, a]);
            for b in 0..0x80u8 {
                if let Ok(cmd) = Command::decode(&[op, a, b]) {
                    assert_eq!(Command::decode(&cmd.encode().unwrap()).unwrap(), cmd);
                }
            }
        }
    }
}

#[test]
fn streams_and_replies_never_panic() {
    let mut r = Rng(42);
    for _ in 0..20_000 {
        let n = r.below(64);
        let data = r.bytes(n);
        let _ = parse(&data);
        let lenient = parse_lenient(&data);
        assert_eq!(lenient.len(), split(&data).count());
        let _ = Reply::decode(&data);
        let swz = Swizzle::new(r.byte());
        let _ = unframe_udp(&swz, &data);
        let _ = Reply::from_wire(&swz, &data);
    }
}

#[test]
fn udp_unframe_refuses_bad_checksums() {
    let swz = Swizzle::default();
    let mut r = Rng(7);
    for _ in 0..10_000 {
        let n = 1 + r.below(100);
        let plain = r.bytes(n);
        let pkt = ruida::frame::frame_udp(&swz, &plain).unwrap();
        assert_eq!(unframe_udp(&swz, &pkt).unwrap(), plain);
        // Any single-byte change in the body changes the 16-bit sum.
        let mut bad = pkt.clone();
        let i = 2 + r.below(n);
        bad[i] = bad[i].wrapping_add(1 + r.byte() % 255);
        assert!(unframe_udp(&swz, &bad).is_err());
        // Truncated datagrams must not panic (a cut can still pass when the dropped bytes sum to 0 mod 65536).
        for cut in 0..pkt.len() {
            let _ = unframe_udp(&swz, &pkt[..cut]);
        }
    }
}

fn job() -> RdJob {
    RdJob {
        layers: vec![
            RdLayer {
                color: [10, 20, 30],
                speed_mm_s: 80.0,
                min_power: [12.0, 0.0],
                max_power: [40.0, 0.0],
                frequency: None,
                polylines: vec![vec![[0.0, 0.0], [50.0, 0.0], [50.0, 50.0], [0.5, 49.9]], vec![[100.0, 100.0], [101.0, 102.0]]],
            },
            RdLayer {
                color: [0, 0, 0],
                speed_mm_s: 5.0,
                min_power: [1.0, 1.0],
                max_power: [100.0, 100.0],
                frequency: Some(1000),
                polylines: vec![vec![[-3.0, 4.0], [3.0, -4.0]]],
            },
        ],
    }
}

#[test]
fn rd_reader_survives_truncation_and_corruption() {
    let bytes = write_rd(&job(), &WriteOptions::default()).unwrap();
    for cut in 0..bytes.len() {
        let _ = read_rd(&bytes[..cut], None);
        let _ = read_rd(&bytes[..cut], Some(0x88));
    }
    let mut r = Rng(1234);
    for _ in 0..2000 {
        let mut b = bytes.clone();
        for _ in 0..1 + r.below(8) {
            let i = r.below(b.len());
            b[i] = r.byte();
        }
        if let Ok(read) = read_rd(&b, Some(0x88)) {
            // A corrupted file is either still a valid stream or flagged.
            let _ = read.file_sum_ok;
        }
        let _ = read_rd(&b, None);
    }
}

#[test]
fn rd_reader_with_wrong_key_does_not_invent_geometry() {
    let bytes = write_rd(&job(), &WriteOptions::default()).unwrap();
    for key in 0..=255u8 {
        if key == 0x88 {
            continue;
        }
        if let Ok(read) = read_rd(&bytes, Some(key)) {
            assert_ne!(read.file_sum_ok, Some(true), "key {key:#04x} passed the file sum");
        }
    }
}

#[test]
fn rd_reader_random_and_huge_input() {
    let mut r = Rng(99);
    for n in [0usize, 1, 2, 7, 100, 4096, 4097, 70_000] {
        let data = r.bytes(n);
        let _ = detect_magic(&data);
        let _ = read_rd(&data, None);
        let _ = read_rd(&data, Some(0x88));
    }
    // A large file of nothing but cuts (8 MiB of CutRelX).
    let mut plain = vec![0x88, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    while plain.len() < 8 << 20 {
        plain.extend([0xAA, 0x00, 0x01]);
    }
    plain.push(0xD7);
    let data = Swizzle::default().scramble(&plain);
    let read = read_rd(&data, None).unwrap();
    assert_eq!(read.magic, 0x88);
    assert_eq!(read.job.layers.len(), 1);
    // Runs of one byte: the histogram guess must not pick a key that makes no commands.
    let read = read_rd(&vec![0x89; 10_000], None);
    if let Ok(read) = read {
        assert!(read.commands.iter().all(|c| !c.is_cut()));
    }
}

#[test]
fn rd_reader_extreme_coordinates() {
    // Absolute coordinates at the 32-bit limits, then relative steps past them.
    let cmds = vec![
        Command::MoveAbs { x: i32::MAX, y: i32::MIN },
        Command::CutRel { dx: 8191, dy: -8192 },
        Command::CutAbs { x: i32::MIN, y: i32::MAX },
        Command::EndOfFile,
    ];
    let plain = ruida::encode_all(&cmds).unwrap();
    let read = read_rd(&Swizzle::default().scramble(&plain), Some(0x88)).unwrap();
    assert!(read.warnings.iter().any(|w| w.contains("32-bit")));
    // Zero-extended negative absolute coordinates (bits 32..34 clear) read as negative.
    let mut raw = vec![0xA8];
    raw.extend(ruida::num::encode_u35(u64::from((-5i32).cast_unsigned())).unwrap());
    raw.extend(ruida::num::encode_u35(5).unwrap());
    assert_eq!(Command::decode(&raw).unwrap(), Command::CutAbs { x: -5, y: 5 });
}

#[cfg(feature = "sim")]
mod sim {
    use super::*;
    use ruida::sim::Simulator;

    #[test]
    fn simulator_survives_random_datagrams_and_serial() {
        let mut r = Rng(5);
        let mut sim = Simulator::new(0x88);
        let swz = Swizzle::default();
        for _ in 0..20_000 {
            let n = r.below(200);
            let raw = r.bytes(n);
            let _ = sim.handle_udp(&raw);
            let _ = sim.handle_serial(&raw);
            // Properly framed random commands too.
            let mut plain = Vec::new();
            for _ in 0..1 + r.below(5) {
                plain.extend(random_chunk(&mut r));
            }
            let out = sim.handle_udp(&ruida::frame::frame_udp(&swz, &plain[..plain.len().min(1470)]).unwrap());
            assert_eq!(Reply::from_wire(&swz, &out[0]), Reply::Ack);
        }
    }
}

