//! Synthetic transcript generator for honest benchmarking.
//!
//! Produces a corpus shaped like real Claude Code data: heavy content
//! payloads (which dominate file size but must be skipped by the parser),
//! duplicate streaming records, user/attachment records, subagent files,
//! and the occasional malformed line. Deterministic — same args, same
//! corpus.
//!
//! Usage: `cargo run --release --example generate_fixtures -- <DIR> <MB>`

use std::io::Write;
use std::path::Path;

use anyhow::Context;

/// Small deterministic PRNG (SplitMix64); no rand dependency needed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo + 1)
    }
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = args.next().context("usage: generate_fixtures <DIR> <MB>")?;
    let target_mb: u64 = args
        .next()
        .context("usage: generate_fixtures <DIR> <MB>")?
        .parse()
        .context("MB must be a number")?;
    let target_bytes = target_mb * 1024 * 1024;

    let root = Path::new(&dir);
    let mut rng = Rng(42);
    let mut written: u64 = 0;
    let mut project = 0;
    while written < target_bytes {
        project += 1;
        let project_dir = root.join(format!("-Users-bench-Projects-app-{project:03}"));
        for session in 0..rng.range(2, 6) {
            written += write_session(&project_dir, session, &mut rng)?;
            if written >= target_bytes {
                break;
            }
        }
    }
    println!(
        "wrote {} MB across {project} projects",
        written / 1024 / 1024
    );
    Ok(())
}

/// One session file plus, sometimes, a subagent file. Returns bytes written.
fn write_session(project_dir: &Path, session: u64, rng: &mut Rng) -> anyhow::Result<u64> {
    std::fs::create_dir_all(project_dir)?;
    let session_id = format!("sess-{session:08}-{:08x}", rng.next() as u32);
    let mut bytes = write_transcript(
        &project_dir.join(format!("{session_id}.jsonl")),
        &session_id,
        rng.range(200, 800),
        rng,
    )?;
    if rng.range(0, 2) == 0 {
        let sub = project_dir.join(format!("{session_id}/subagents"));
        std::fs::create_dir_all(&sub)?;
        bytes += write_transcript(
            &sub.join(format!("agent-a{:016x}.jsonl", rng.next())),
            &session_id,
            rng.range(50, 200),
            rng,
        )?;
    }
    Ok(bytes)
}

fn write_transcript(
    path: &Path,
    session_id: &str,
    messages: u64,
    rng: &mut Rng,
) -> anyhow::Result<u64> {
    const MODELS: [&str; 3] = ["claude-opus-4-8", "claude-sonnet-5", "claude-haiku-4-5"];
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let mut bytes: u64 = 0;
    let mut day_seconds = 0;
    for message in 0..messages {
        day_seconds += rng.range(1, 60);
        let ts = format!(
            "2026-{:02}-{:02}T{:02}:{:02}:{:02}.000Z",
            rng.range(1, 7),
            rng.range(1, 28),
            (day_seconds / 3600) % 24,
            (day_seconds / 60) % 60,
            day_seconds % 60
        );
        // User prompt with content the parser must skip over.
        let user_pad = "u".repeat(rng.range(100, 2_000) as usize);
        bytes += write_line(
            &mut file,
            &format!(
                r#"{{"type":"user","uuid":"u-{session_id}-{message}","timestamp":"{ts}","sessionId":"{session_id}","message":{{"role":"user","content":"{user_pad}"}}}}"#
            ),
        )?;
        // Assistant record(s): content is the bulk, usage is what matters.
        // ~20% of messages stream 2-4 duplicate snapshots.
        let model = MODELS[(rng.next() % 3) as usize];
        let content_pad = "x".repeat(rng.range(1_000, 8_000) as usize);
        let output = rng.range(50, 6_000);
        let snapshots = if rng.range(0, 4) == 0 {
            rng.range(2, 4)
        } else {
            1
        };
        for snapshot in 0..snapshots {
            bytes += write_line(
                &mut file,
                &format!(
                    r#"{{"type":"assistant","uuid":"a-{session_id}-{message}-{snapshot}","timestamp":"{ts}","sessionId":"{session_id}","requestId":"req_{session_id}_{message}","message":{{"id":"msg_{session_id}_{message}","model":"{model}","content":[{{"type":"text","text":"{content_pad}"}}],"usage":{{"input_tokens":{},"output_tokens":{output},"cache_creation_input_tokens":{},"cache_read_input_tokens":{},"cache_creation":{{"ephemeral_5m_input_tokens":{},"ephemeral_1h_input_tokens":{}}}}}}}}}"#,
                    rng.range(1, 500),
                    rng.range(0, 10_000),
                    rng.range(10_000, 900_000),
                    rng.range(0, 8_000),
                    rng.range(0, 4_000),
                ),
            )?;
        }
        // Occasional noise: unknown record types and a rare malformed line.
        if rng.range(0, 9) == 0 {
            bytes += write_line(
                &mut file,
                &format!(r#"{{"type":"file-history-snapshot","uuid":"f-{message}"}}"#),
            )?;
        }
        if rng.range(0, 499) == 0 {
            bytes += write_line(&mut file, "not json at all %%%")?;
        }
    }
    Ok(bytes)
}

fn write_line(file: &mut impl Write, line: &str) -> anyhow::Result<u64> {
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(line.len() as u64 + 1)
}
