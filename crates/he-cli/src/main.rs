//! Debug client for the HIT_ENTER protocol.
//!
//! Dropping HTTP for raw QUIC streams cost us the ability to poke at the server
//! with `curl`. This binary is the replacement, and it exists from M0 so that
//! nobody is ever tempted to debug the protocol with print statements.

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    println!("he-cli — HIT_ENTER debug client");
    println!("protocol : {}", String::from_utf8_lossy(he_proto::ALPN));
    println!("version  : {}", he_proto::PROTOCOL_VERSION);
    println!();
    println!("M0 skeleton: dialing arrives in M2 (see docs/PLAN.md).");

    Ok(())
}
