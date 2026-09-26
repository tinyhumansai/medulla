//! Decode one `TrackedTask` JSON value through medulla's Rust harness mirror.
//!
//! This deliberately tiny example is the executable seam used by the
//! integration umbrella's cross-repository contract test. It reads one JSON
//! value from stdin, fails non-zero if serde rejects it, and writes the
//! canonical re-serialized value to stdout.

use std::io::{self, Read};

use medulla::harness_contract::TrackedTask;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let task: TrackedTask = serde_json::from_str(&input)?;
    println!("{}", serde_json::to_string(&task)?);
    Ok(())
}
