//! Headless HIT_ENTER server.
//!
//! The same `he-server` library the desktop app hosts with — no GUI, so it can
//! run on a NAS, a spare box or a VPS. It binds an iroh endpoint and serves
//! `hit-enter/0` on it: no port to forward, no certificate to obtain, no DNS
//! name to buy (PLAN §4).
//!
//! The address it prints is a public key. That is the whole address.

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use he_proto::Password;
use he_server::Server;

const USAGE: &str = "\
he-serverd — headless HIT_ENTER server

USAGE:
    he-serverd [OPTIONS]

OPTIONS:
    -d, --data-dir <PATH>  Where server.db lives  [env: HE_DATA_DIR] [default: ./he-data]
    -n, --name <NAME>      Space name, used only when creating a new database
        --owner <NAME>     Create the owner account and exit. Only works once,
                           and only here: registration over the network is
                           always invite-gated, and the first account cannot be.
        --invite           Mint an invite code, print it, and exit
        --max-uses <N>     With --invite: how many people may use it [default: 1]
        --expires-in <S>   With --invite: seconds until it expires [default: never]
    -h, --help             Print this message

PASSWORDS
    Read from the HE_PASSWORD environment variable, or from stdin if it is not
    set. Typing one at a terminal echoes it; pipe it in or use the variable.
";

#[derive(Default)]
struct Args {
    data_dir: Option<PathBuf>,
    name: Option<String>,
    owner: Option<String>,
    invite: bool,
    max_uses: Option<i64>,
    expires_in: Option<u64>,
}

fn parse_args() -> Result<Option<Args>> {
    let mut parsed = Args {
        data_dir: std::env::var_os("HE_DATA_DIR").map(PathBuf::from),
        ..Args::default()
    };
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "-d" | "--data-dir" => {
                parsed.data_dir = Some(PathBuf::from(
                    args.next().context("--data-dir needs a path")?,
                ));
            }
            "-n" | "--name" => parsed.name = Some(args.next().context("--name needs a value")?),
            "--owner" => parsed.owner = Some(args.next().context("--owner needs a username")?),
            "--invite" => parsed.invite = true,
            "--max-uses" => {
                parsed.max_uses = Some(
                    args.next()
                        .context("--max-uses needs a number")?
                        .parse()
                        .context("--max-uses must be a number")?,
                );
            }
            "--expires-in" => {
                parsed.expires_in = Some(
                    args.next()
                        .context("--expires-in needs a number of seconds")?
                        .parse()
                        .context("--expires-in must be a number of seconds")?,
                );
            }
            other => anyhow::bail!("unrecognised argument {other:?}\n\n{USAGE}"),
        }
    }

    Ok(Some(parsed))
}

/// Reads a password from the environment or stdin.
///
/// Never from a command-line argument: `ps` shows those to every account on
/// the machine, and shells write them to a history file.
fn read_password(prompt: &str) -> Result<Password> {
    if let Ok(password) = std::env::var("HE_PASSWORD") {
        return Ok(Password::new(password));
    }

    let mut stderr = std::io::stderr();
    if stderr.is_terminal() {
        write!(stderr, "{prompt} (will be echoed): ")?;
        stderr.flush()?;
    }

    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .context("reading a password from stdin")?;
    Ok(Password::new(line.trim_end_matches(['\r', '\n'])))
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

    let data_dir = args.data_dir.unwrap_or_else(|| PathBuf::from("./he-data"));
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating {}", data_dir.display()))?;
    let path = data_dir.join("server.db");

    let name = args.name.unwrap_or_else(|| "HIT_ENTER".to_string());
    let server = Arc::new(Server::open(&path, &name).await?);

    if let Some(username) = args.owner {
        let password = read_password("owner password")?;
        let user = server.create_owner(&username, &password).await?;
        println!("owner account created: {} ({})", user.username, user.id);
        println!("run again without --owner to start serving");
        return Ok(());
    }

    if args.invite {
        let owner = owner_account(&server).await?;
        let invite = server
            .create_invite(
                &owner.id,
                args.expires_in.map(std::time::Duration::from_secs),
                args.max_uses.or(Some(1)),
            )
            .await?;
        println!("invite code : {}", invite.code);
        println!("endpoint id : {}", server.endpoint_id());
        println!();
        println!("The code and the endpoint id are both needed to join.");
        return Ok(());
    }

    // The EndpointId is the whole address: no IP, no port, no DNS name. It is
    // public by design — it is what an invite ticket carries (PLAN §5).
    println!("space       : {}", server.name());
    println!("database    : {}", path.display());
    println!("endpoint id : {}", server.endpoint_id());
    println!();

    let router = he_server::serve(server).await?;
    tracing::info!("serving {}", String::from_utf8_lossy(he_proto::ALPN));
    println!("listening. ^C to stop.");

    tokio::signal::ctrl_c()
        .await
        .context("waiting for ctrl-c")?;

    println!();
    tracing::info!("shutting down");
    // Lets in-flight sessions finish rather than dropping the endpoint from
    // under them, which on the other end looks like a crash.
    router.shutdown().await.ok();
    Ok(())
}

/// The account an invite is recorded against.
///
/// Invites made from the command line belong to the owner: the person running
/// this process is the one who owns the machine and the database.
async fn owner_account(server: &Server) -> Result<he_server::User> {
    for member in server.members().await? {
        if member.is_owner {
            return server
                .user_by_id(&member.id)
                .await?
                .context("the owner account vanished between two queries");
        }
    }
    anyhow::bail!("this space has no owner yet — run with --owner <username> first")
}
