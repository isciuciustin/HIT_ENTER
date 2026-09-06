//! Headless HIT_ENTER server.
//!
//! Same `he-server` library the desktop app hosts with — no GUI, so it can run
//! on a NAS, a spare box or a VPS.

use anyhow::Result;
use he_server::ServerIdentity;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "he_serverd=info,he_server=info".into()),
        )
        .init();

    // M0: prove the identity primitive works end to end. M1 persists this in
    // SQLite instead of generating a throwaway on every start.
    let identity = ServerIdentity::generate();
    tracing::info!(endpoint_id = %identity.endpoint_id(), "HIT_ENTER server identity");
    tracing::warn!("M0 skeleton: not yet accepting connections (see docs/PLAN.md M2)");

    Ok(())
}
