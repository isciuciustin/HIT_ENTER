//! Headless HIT_ENTER server.
//!
//! Same `he-server` library the desktop app hosts with — no GUI, so it can run
//! on a NAS, a spare box or a VPS.
//!
//! M1 status: this opens (and on a first run creates) the space's database and
//! loads its identity, which is the address invites point at. It does not yet
//! accept connections — the iroh `Endpoint` and the protocol handler are M2.

use std::path::PathBuf;

use anyhow::{Context, Result};
use he_server::Server;

const USAGE: &str = "\
he-serverd — headless HIT_ENTER server

USAGE:
    he-serverd [OPTIONS]

OPTIONS:
    -d, --data-dir <PATH>  Where server.db lives  [env: HE_DATA_DIR] [default: ./he-data]
    -n, --name <NAME>      Space name, used only when creating a new database
    -h, --help             Print this message
";

struct Args {
    data_dir: PathBuf,
    name: String,
}

fn parse_args() -> Result<Option<Args>> {
    let mut data_dir = std::env::var_os("HE_DATA_DIR").map(PathBuf::from);
    let mut name = None;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "-d" | "--data-dir" => {
                data_dir = Some(PathBuf::from(
                    args.next().context("--data-dir needs a path")?,
                ));
            }
            "-n" | "--name" => {
                name = Some(args.next().context("--name needs a value")?);
            }
            other => anyhow::bail!("unrecognised argument {other:?}\n\n{USAGE}"),
        }
    }

    Ok(Some(Args {
        data_dir: data_dir.unwrap_or_else(|| PathBuf::from("./he-data")),
        name: name.unwrap_or_else(|| "HIT_ENTER".to_string()),
    }))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "he_serverd=info,he_server=info".into()),
        )
        .init();

    let Some(args) = parse_args()? else {
        return Ok(());
    };

    std::fs::create_dir_all(&args.data_dir)
        .with_context(|| format!("creating {}", args.data_dir.display()))?;
    let path = args.data_dir.join("server.db");

    let server = Server::open(&path, &args.name).await?;

    // The EndpointId is the whole address: no IP, no port, no DNS name. It is
    // public by design — it is what an invite ticket carries (PLAN §5).
    println!("space      : {}", server.name());
    println!("database   : {}", path.display());
    println!("endpoint id: {}", server.endpoint_id());

    tracing::warn!("M1: the database is live but nothing is listening yet (see docs/PLAN.md M2)");
    Ok(())
}
