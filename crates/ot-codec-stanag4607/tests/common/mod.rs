//! Byte builders for hand-made packets (layouts from Annex A tables).
#![allow(dead_code)]

use std::path::{Path, PathBuf};

#[derive(Default, Clone)]
pub struct B(pub Vec<u8>);

impl B {
    pub fn new() -> Self {
        Self(Vec::new())
    }
    pub fn u8(mut self, v: u8) -> Self {
        self.0.push(v);
        self
    }
    pub fn i8(self, v: i8) -> Self {
        self.u8(v.to_be_bytes()[0])
    }
    pub fn u16(mut self, v: u16) -> Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    pub fn i16(mut self, v: i16) -> Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    pub fn u32(mut self, v: u32) -> Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    pub fn i32(mut self, v: i32) -> Self {
        self.0.extend(v.to_be_bytes());
        self
    }
    pub fn bytes(mut self, v: &[u8]) -> Self {
        self.0.extend_from_slice(v);
        self
    }
    /// Alphanumeric field padded with BCS spaces.
    pub fn a(mut self, s: &str, n: usize) -> Self {
        let mut v = s.as_bytes().to_vec();
        v.resize(n, b' ');
        self.0.extend(v);
        self
    }
    /// SA32 from degrees.
    pub fn sa32(self, deg: f64) -> Self {
        self.i32((deg * 4_294_967_296.0 / 180.0).round() as i32)
    }
    /// BA32 from degrees (0..360).
    pub fn ba32(self, deg: f64) -> Self {
        self.u32((deg.rem_euclid(360.0) * 4_294_967_296.0 / 360.0).round() as u32)
    }
}

/// A segment: 5-byte header (type, size) plus body.
pub fn segment(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut v = vec![kind];
    v.extend(((body.len() + 5) as u32).to_be_bytes());
    v.extend_from_slice(body);
    v
}

/// A packet: 32-byte header (Table 2-1) plus segments.
pub fn packet(job_id: u32, segments: &[Vec<u8>]) -> Vec<u8> {
    let body: Vec<u8> = segments.concat();
    let mut v = B::new()
        .a("30", 2)
        .u32((32 + body.len()) as u32)
        .a("XN", 2)
        .u8(5)
        .a("XN", 2)
        .u16(0x2001)
        .u8(129)
        .a("TAIL01", 10)
        .u32(77)
        .u32(job_id)
        .0;
    v.extend(body);
    v
}

pub fn mission_body() -> Vec<u8> {
    B::new()
        .a("MSN-PLAN-1", 12)
        .a("FP-7", 12)
        .u8(21)
        .a("CFG 2.1", 10)
        .u16(2024)
        .u8(2)
        .u8(28)
        .0
}

pub fn job_definition_body(job_id: u32) -> Vec<u8> {
    B::new()
        .u32(job_id)
        .u8(13) // MP-RTIP
        .a("MOD-A", 6)
        .u8(0b101) // area filtering + sector blanking
        .u8(10)
        .sa32(10.0)
        .ba32(20.0)
        .sa32(11.0)
        .ba32(20.0)
        .sa32(11.0)
        .ba32(21.0)
        .sa32(10.0)
        .ba32(21.0)
        .u8(1) // MTI
        .u16(125) // 12.5 s
        .u16(50) // 5.0 m
        .u16(0xFFFF) // no statement
        .u16(30) // 3.0 m
        .u8(2)
        .u16(250) // 0.25 m/s
        .u16(1500) // 15 m
        .u16(0x0100) // BA16 1.40625 deg
        .u16(40) // 0.4 m/s
        .u8(15) // 1.5 m/s
        .u8(85)
        .u8(60)
        .u8(3) // DTED2
        .u8(1) // EGM96
        .0
}

/// Sample corpus location: `OT_GMTI_SAMPLES`, default `$HOME/data/gmti/STANAG4607`.
fn sample_dir() -> PathBuf {
    std::env::var_os("OT_GMTI_SAMPLES").map_or_else(
        || {
            let home = std::env::var_os("HOME").unwrap_or_default();
            Path::new(&home).join("data/gmti/STANAG4607")
        },
        PathBuf::from,
    )
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if p
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("4607"))
        {
            out.push(p);
        }
    }
}

/// The sample files (sorted), or None (with a message) when the corpus is absent.
pub fn samples() -> Option<Vec<(PathBuf, Vec<u8>)>> {
    let dir = sample_dir();
    let mut files = Vec::new();
    collect(&dir, &mut files);
    files.sort();
    if files.is_empty() {
        eprintln!("skipping: no STANAG 4607 samples under {}", dir.display());
        return None;
    }
    Some(
        files
            .into_iter()
            .map(|p| {
                let b = std::fs::read(&p).expect("read sample");
                (p, b)
            })
            .collect(),
    )
}
