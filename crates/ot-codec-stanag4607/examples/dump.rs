//! Print a recording's records as JSON lines: `cargo run --example dump --
//! <file.4607> [targets|segments|all]`. For looking at recordings.

use std::io::Write;

use ot_codec_stanag4607::{Decoder, Emit, Options, packet_len};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: dump <file.4607> [targets|segments|all]");
    let emit = match args.next().as_deref() {
        None | Some("targets") => Emit::Targets,
        Some("segments") => Emit::Segments,
        Some("all") => Emit::All,
        Some(other) => panic!("unknown emit {other}"),
    };
    let bytes = std::fs::read(&path).expect("read");
    let mut d = Decoder::new(Options {
        emit,
        ..Default::default()
    });
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    let mut i = 0;
    while i < bytes.len() {
        let n = packet_len(&bytes[i..]).expect("packet");
        for r in d.decode(&bytes[i..i + n]).expect("decode") {
            // A closed pipe (`| head`) ends the dump quietly.
            if writeln!(out, "{r}").is_err() {
                return;
            }
        }
        i += n;
    }
}
