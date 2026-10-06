//! No panics on malformed input: prefix truncations and random byte flips,
//! over real samples when available and over hand-built packets always.

mod common;

use common::{B, job_definition_body, mission_body, packet, segment};
use ot_codec_stanag4607::{Decoder, Emit, Options, packet_len, parse_packet};

/// xorshift64*: deterministic, dependency-free.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn hand_built_stream() -> Vec<u8> {
    let dwell = B::new()
        .bytes(&[0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0])
        .u16(1)
        .u16(2)
        .u8(0)
        .u16(2) // two target reports
        .bytes(&[0x11; 71]) // rest of D6-D31 (78 bytes in all)
        .bytes(&[0x22; 2 * 36]) // two reports with every D32 field
        .0;
    let hrr = B::new()
        .bytes(&[0xFF, 0xFF, 0xFF, 0xFF, 0xC0])
        .bytes(&[0x01; 61 + 2 * 6]) // H2-H31, then two 6-byte scatterers
        .0;
    let mut s = packet(
        0,
        &[
            segment(1, &mission_body()),
            segment(5, &job_definition_body(5)),
        ],
    );
    s.extend(packet(5, &[segment(2, &dwell), segment(3, &hrr)]));
    s
}

/// A few whole packets from the start of a real sample (mission, job, dwells).
fn real_prefix() -> Option<Vec<u8>> {
    let files = common::samples()?;
    let bytes = &files.first()?.1;
    let mut end = 0;
    for _ in 0..6 {
        end += packet_len(bytes.get(end..)?)?;
    }
    bytes.get(..end).map(<[u8]>::to_vec)
}

fn check_truncations(stream: &[u8]) {
    for emit in [Emit::Targets, Emit::All] {
        let full = Decoder::new(Options {
            emit,
            platform: true,
            ..Default::default()
        })
        .decode(stream)
        .unwrap();
        assert!(full.iter().all(|r| r["record"] != "warning"), "{full:?}");
        assert!(full.iter().any(|r| r["record"] == "target"));
        for n in 0..stream.len() {
            let prefix = &stream[..n];
            if let Ok(recs) = Decoder::new(Options {
                emit,
                platform: true,
                ..Default::default()
            })
            .decode(prefix)
            {
                assert!(recs.len() <= full.len(), "prefix {n}");
            }
            let _ = parse_packet(prefix);
            if let (Some(len), Ok((_, used))) = (packet_len(prefix), parse_packet(prefix)) {
                assert_eq!(len, used);
            }
        }
    }
}

fn check_flips(stream: &[u8], seed: u64, iterations: usize) {
    let mut rng = Rng(seed);
    for i in 0..iterations {
        let mut b = stream.to_vec();
        for _ in 0..1 + rng.below(4) {
            let at = rng.below(b.len());
            b[at] ^= 1 << rng.below(8);
            if i % 3 == 0 {
                b[at] = rng.next() as u8;
            }
        }
        let emit = [Emit::Targets, Emit::Segments, Emit::All][i % 3];
        let _ = Decoder::new(Options {
            emit,
            platform: true,
            ..Default::default()
        })
        .decode(&b);
    }
}

#[test]
fn truncation_never_panics() {
    check_truncations(&hand_built_stream());
    if let Some(real) = real_prefix() {
        check_truncations(&real);
    }
}

#[test]
fn random_flips_never_panic() {
    check_flips(&hand_built_stream(), 0x5EED_1234, 3000);
    if let Some(real) = real_prefix() {
        check_flips(&real, 0xC0FF_EE00, 4000);
    }
}

#[test]
fn garbage_never_panics() {
    let mut rng = Rng(42);
    for _ in 0..2000 {
        let len = rng.below(300);
        let mut b: Vec<u8> = (0..len).map(|_| rng.next() as u8).collect();
        // Make some of them look like plausible packets.
        if len >= 32 && rng.below(2) == 0 {
            b[2..6].copy_from_slice(&(len as u32).to_be_bytes());
        }
        let _ = Decoder::new(Options {
            emit: Emit::All,
            platform: true,
            ..Default::default()
        })
        .decode(&b);
        let _ = packet_len(&b);
    }
}
