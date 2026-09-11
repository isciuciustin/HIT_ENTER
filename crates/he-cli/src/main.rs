//! Debug client for the HIT_ENTER protocol.
//!
//! Dropping HTTP for raw QUIC streams cost us the ability to poke at a server
//! with `curl`. This is the replacement, and it exists so that nobody is ever
//! tempted to debug a binary protocol with print statements.
//!
//! It keeps a device key on disk like the real client does, so the first
//! command costs a password and every later one does not — which makes it the
//! shortest way to check that device enrolment still works (PLAN §3).

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use he_client::{Client, DeviceIdentity, Session};
use he_proto::rpc::Auth;
use he_proto::{Password, ServerFrame};
use iroh::{EndpointAddr, EndpointId};

const USAGE: &str = "\
he-cli — HIT_ENTER debug client

USAGE:
    he-cli --server <ENDPOINT_ID> [OPTIONS] <COMMAND>

COMMANDS:
    info                        Connect, print what the server said, disconnect
    send <CHANNEL> <TEXT>       Post a message
    history [CHANNEL]           Print recent messages
    watch                       Print events until ^C
    invite [--max-uses N]       Mint an invite code

OPTIONS:
    -s, --server <ENDPOINT_ID>  Who to dial      [env: HE_SERVER]
    -k, --key <PATH>            Device key file  [default: ./he-cli.key]
    -u, --username <NAME>       Log in as this account
    -i, --invite <CODE>         Register with this invite code
        --addr <IP:PORT>        Skip discovery and dial this socket directly
        --max-uses <N>          With `invite` [default: 1]
        --expires-in <S>        With `invite` [default: never]
    -h, --help                  Print this message

AUTHENTICATION
    With neither --invite nor --username, the device key is the whole login and
    no password is needed. With --username a password enrols this key; with
    --invite it creates the account first. Passwords come from HE_PASSWORD or
    stdin, never from an argument — `ps` shows arguments to everyone.

CHANNELS
    A channel may be given by name or by id. With no channel, `history` uses
    the first one the server listed.
";

struct Args {
    server: EndpointId,
    key: PathBuf,
    username: Option<String>,
    invite: Option<String>,
    addr: Option<std::net::SocketAddr>,
    max_uses: Option<i64>,
    expires_in: Option<u64>,
    command: Command,
}

enum Command {
    Info,
    Send { channel: String, text: String },
    History { channel: Option<String> },
    Watch,
    Invite,
}

fn parse_args() -> Result<Option<Args>> {
    let mut server = std::env::var("HE_SERVER").ok();
    let mut key = None;
    let mut username = None;
    let mut invite = None;
    let mut addr = None;
    let mut max_uses = None;
    let mut expires_in = None;
    let mut rest: Vec<String> = Vec::new();
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "-s" | "--server" => server = Some(args.next().context("--server needs a value")?),
            "-k" | "--key" => key = Some(PathBuf::from(args.next().context("--key needs a path")?)),
            "-u" | "--username" => {
                username = Some(args.next().context("--username needs a value")?)
            }
            "-i" | "--invite" => invite = Some(args.next().context("--invite needs a code")?),
            "--addr" => {
                addr = Some(
                    args.next()
                        .context("--addr needs a socket address")?
                        .parse()
                        .context("--addr must be IP:PORT")?,
                )
            }
            "--max-uses" => {
                max_uses = Some(
                    args.next()
                        .context("--max-uses needs a number")?
                        .parse()
                        .context("--max-uses must be a number")?,
                )
            }
            "--expires-in" => {
                expires_in = Some(
                    args.next()
                        .context("--expires-in needs seconds")?
                        .parse()
                        .context("--expires-in must be a number of seconds")?,
                )
            }
            other if other.starts_with('-') => {
                bail!("unrecognised option {other:?}\n\n{USAGE}")
            }
            other => rest.push(other.to_owned()),
        }
    }

    let command = match rest.split_first() {
        None => bail!("no command given\n\n{USAGE}"),
        Some((verb, tail)) => match verb.as_str() {
            "info" => Command::Info,
            "watch" => Command::Watch,
            "invite" => Command::Invite,
            "history" => Command::History {
                channel: tail.first().cloned(),
            },
            "send" => {
                let channel = tail.first().context("send needs a channel")?.clone();
                let text = tail.get(1..).unwrap_or_default().join(" ");
                if text.is_empty() {
                    bail!("send needs something to say");
                }
                Command::Send { channel, text }
            }
            other => bail!("unrecognised command {other:?}\n\n{USAGE}"),
        },
    };

    let server = server.context("--server is required (or set HE_SERVER)")?;
    let server: EndpointId = server
        .parse()
        .context("--server must be an EndpointId, as printed by he-serverd")?;

    Ok(Some(Args {
        server,
        key: key.unwrap_or_else(|| PathBuf::from("./he-cli.key")),
        username,
        invite,
        addr,
        max_uses,
        expires_in,
        command,
    }))
}

/// Reads a password from the environment or stdin, never from an argument.
fn read_password() -> Result<Password> {
    use std::io::{IsTerminal, Write};

    if let Ok(password) = std::env::var("HE_PASSWORD") {
        return Ok(Password::new(password));
    }
    let mut stderr = std::io::stderr();
    if stderr.is_terminal() {
        write!(stderr, "password (will be echoed): ")?;
        stderr.flush()?;
    }
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(Password::new(line.trim_end_matches(['\r', '\n'])))
}

fn auth_for(args: &Args) -> Result<Auth> {
    match (&args.invite, &args.username) {
        (Some(invite), Some(username)) => Ok(Auth::Register {
            invite: invite.clone(),
            username: username.clone(),
            password: read_password()?,
        }),
        (Some(_), None) => bail!("--invite also needs --username: a new account needs a name"),
        (None, Some(username)) => Ok(Auth::Password {
            username: username.clone(),
            password: read_password()?,
        }),
        // The whole point of enrolment: this key is the login (PLAN §3).
        (None, None) => Ok(Auth::Device { username: None }),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "he_cli=info,he_client=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    let Some(args) = parse_args()? else {
        return Ok(());
    };

    let identity = DeviceIdentity::load_or_create(&args.key)?;
    eprintln!("device id : {}", identity.endpoint_id());

    let client = Client::bind(&identity).await?;

    // A bare EndpointId is resolved by discovery. `--addr` skips that, which
    // is how you test on a LAN with the internet unplugged.
    let addr = match args.addr {
        Some(socket) => EndpointAddr::from_parts(args.server, [iroh::TransportAddr::Ip(socket)]),
        None => EndpointAddr::new(args.server),
    };

    let mut session = client.connect(addr, auth_for(&args)?).await?;
    eprintln!(
        "connected : {} as {} ({:?})",
        session.ready().server_name,
        session.ready().user.username,
        session.path()
    );

    match &args.command {
        Command::Info => print_info(&session),
        Command::Send { channel, text } => {
            let channel_id = resolve_channel(&session, Some(channel))?;
            let nonce = session.send_message(&channel_id, text).await?;
            // Wait for the echo: the send is only confirmed once the server
            // has stored it and told us so (PLAN §9).
            await_echo(&mut session, &nonce).await?;
        }
        Command::History { channel } => {
            let channel_id = resolve_channel(&session, channel.as_ref())?;
            let messages = session
                .backfill(&channel_id, None, Session::BACKFILL_LIMIT)
                .await?;
            // Newest first on the wire; oldest first on a terminal, because
            // that is the direction a conversation reads.
            for message in messages.iter().rev() {
                println!("{:>12}  {}", message.author_name, message.content);
            }
        }
        Command::Watch => {
            eprintln!("watching. ^C to stop.");
            watch(&mut session).await;
        }
        Command::Invite => {
            let code = session
                .create_invite(args.expires_in, args.max_uses.or(Some(1)))
                .await?;
            println!("{code}");
        }
    }

    session.close().await;
    client.shutdown().await;
    Ok(())
}

fn print_info(session: &Session) {
    let ready = session.ready();
    println!("space    : {}", ready.server_name);
    println!("account  : {} ({})", ready.user.username, ready.user.id);
    println!("path     : {:?}", session.path());
    println!("channels :");
    for channel in &ready.channels {
        println!("  {}  {}", channel.id, channel.name);
    }
    println!("members  :");
    for member in &ready.members {
        let owner = if member.is_owner { " (owner)" } else { "" };
        println!("  {}{}", member.username, owner);
    }
}

/// Turns a channel name or id into an id, defaulting to the first channel.
fn resolve_channel(session: &Session, wanted: Option<&String>) -> Result<String> {
    let channels = &session.ready().channels;
    match wanted {
        None => channels
            .first()
            .map(|channel| channel.id.clone())
            .context("this space has no channels"),
        Some(wanted) => channels
            .iter()
            .find(|channel| &channel.id == wanted || &channel.name == wanted)
            .map(|channel| channel.id.clone())
            .with_context(|| format!("no channel called {wanted:?}")),
    }
}

/// Waits for the server to echo our own message back.
async fn await_echo(session: &mut Session, nonce: &str) -> Result<()> {
    let deadline = Duration::from_secs(10);
    loop {
        let frame = tokio::time::timeout(deadline, session.next_event())
            .await
            .context("the server accepted the message but never echoed it")?
            .context("the session ended before the echo arrived")?;

        if let ServerFrame::Message {
            message,
            nonce: echoed,
        } = frame
            && echoed.as_deref() == Some(nonce)
        {
            println!("{}  {}", message.id, message.content);
            return Ok(());
        }
    }
}

async fn watch(session: &mut Session) {
    while let Some(frame) = session.next_event().await {
        match frame {
            ServerFrame::Message { message, nonce } => {
                let mine = if nonce.is_some() { " (mine)" } else { "" };
                println!("{:>12}  {}{}", message.author_name, message.content, mine);
            }
            ServerFrame::Error(err) => {
                eprintln!("server error: {:?}", err.code);
                return;
            }
            ServerFrame::Ready(_) => eprintln!("unexpected second ready frame"),
        }
    }
    eprintln!("session ended");
}
