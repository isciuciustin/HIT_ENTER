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
use he_proto::net::Relays;
use he_proto::rpc::Auth;
use he_proto::{InviteLink, NetworkConfig, Password, ServerFrame};
use iroh::EndpointAddr;

const USAGE: &str = "\
he-cli — HIT_ENTER debug client

USAGE:
    he-cli --server <ADDRESS> [OPTIONS] <COMMAND>

COMMANDS:
    info                        Connect, print what the server said, disconnect
    send <CHANNEL> <TEXT>       Post a message
    edit <ID> <TEXT>            Rewrite one of your own messages
    delete <ID>                 Withdraw one of your own messages
    history [CHANNEL]           Print recent messages
    watch                       Print events until ^C
    invite [--max-uses N]       Mint an invite code

  Owner tools (refused with FORBIDDEN for anyone else):
    members                     List accounts, with who is banned
    devices [USER_ID]           List enrolled devices  [default: your own]
    revoke-device <USER> <KEY>  Kick one machine off
    kick <USER_ID>              Log an account out everywhere
    ban <USER_ID>               Kick, and refuse the password too
    unban <USER_ID>             Let a banned account back in
    invites                     List every invite
    revoke-invite <CODE>        Delete an invite
    channel-add <NAME>          Create a channel
    channel-rm <CHANNEL>        Delete a channel and its messages

OPTIONS:
    -s, --server <ADDRESS>      Who to dial      [env: HE_SERVER]
    -k, --key <PATH>            Device key file  [default: ./he-cli.key]
    -u, --username <NAME>       Log in as this account
    -i, --invite <CODE>         Register with this invite code
        --addr <IP:PORT>        Skip discovery and dial this socket directly
        --max-uses <N>          With `invite` [default: 1]
        --expires-in <S>        With `invite` [default: never]
        --relay <URL>           Use this relay instead of n0's. Repeat for several.
        --no-relay              No relay at all
        --no-dns                Do not use n0's DNS for discovery
        --no-mdns               Do not announce on the local network
        --lan                   Shorthand for --no-relay --no-dns
    -h, --help                  Print this message

ADDRESS
    An EndpointId, an `endpoint…` ticket, or a whole `hitenter://join?…` link.
    A link also carries the invite code, so `--server <link>` alone is enough
    for a first join — `--invite` overrides what the link says.

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
    server: InviteLink,
    key: PathBuf,
    username: Option<String>,
    invite: Option<String>,
    addr: Option<std::net::SocketAddr>,
    max_uses: Option<i64>,
    expires_in: Option<u64>,
    network: NetworkConfig,
    command: Command,
}

enum Command {
    Info,
    Send {
        channel: String,
        text: String,
    },
    Edit {
        id: String,
        text: String,
    },
    Delete {
        id: String,
    },
    History {
        channel: Option<String>,
    },
    Watch,
    Invite,
    Members,
    Devices {
        user_id: Option<String>,
    },
    RevokeDevice {
        user_id: String,
        endpoint_id: String,
    },
    Kick {
        user_id: String,
    },
    SetBanned {
        user_id: String,
        banned: bool,
    },
    Invites,
    RevokeInvite {
        code: String,
    },
    ChannelAdd {
        name: String,
    },
    ChannelRemove {
        channel: String,
    },
}

fn parse_args() -> Result<Option<Args>> {
    let mut server = std::env::var("HE_SERVER").ok();
    let mut key = None;
    let mut username = None;
    let mut invite = None;
    let mut addr = None;
    let mut max_uses = None;
    let mut expires_in = None;
    let mut relays: Vec<String> = Vec::new();
    let mut no_relay = false;
    let mut no_dns = false;
    let mut no_mdns = false;
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
            "--relay" => relays.push(args.next().context("--relay needs a URL")?),
            "--no-relay" => no_relay = true,
            "--no-dns" => no_dns = true,
            "--no-mdns" => no_mdns = true,
            "--lan" => {
                no_relay = true;
                no_dns = true;
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
            "edit" => {
                let id = tail.first().context("edit needs a message id")?.clone();
                let text = tail.get(1..).unwrap_or_default().join(" ");
                if text.is_empty() {
                    bail!("edit needs the new text");
                }
                Command::Edit { id, text }
            }
            "delete" => Command::Delete {
                id: tail.first().context("delete needs a message id")?.clone(),
            },
            "members" => Command::Members,
            "devices" => Command::Devices {
                user_id: tail.first().cloned(),
            },
            "revoke-device" => Command::RevokeDevice {
                user_id: tail
                    .first()
                    .context("revoke-device needs a user id")?
                    .clone(),
                endpoint_id: tail
                    .get(1)
                    .context("revoke-device needs an endpoint id")?
                    .clone(),
            },
            "kick" => Command::Kick {
                user_id: tail.first().context("kick needs a user id")?.clone(),
            },
            "ban" => Command::SetBanned {
                user_id: tail.first().context("ban needs a user id")?.clone(),
                banned: true,
            },
            "unban" => Command::SetBanned {
                user_id: tail.first().context("unban needs a user id")?.clone(),
                banned: false,
            },
            "invites" => Command::Invites,
            "revoke-invite" => Command::RevokeInvite {
                code: tail.first().context("revoke-invite needs a code")?.clone(),
            },
            "channel-add" => Command::ChannelAdd {
                name: tail.first().context("channel-add needs a name")?.clone(),
            },
            "channel-rm" => Command::ChannelRemove {
                channel: tail.first().context("channel-rm needs a channel")?.clone(),
            },
            other => bail!("unrecognised command {other:?}\n\n{USAGE}"),
        },
    };

    let server = server.context("--server is required (or set HE_SERVER)")?;
    let server = InviteLink::parse_relaxed(&server)
        .context("--server must be an EndpointId, a ticket, or a hitenter:// link")?;

    if no_relay && !relays.is_empty() {
        bail!("--relay and --no-relay ask for opposite things");
    }

    Ok(Some(Args {
        // An explicit --invite wins over one carried by the link, so that a
        // stale link can be reused with a fresh code.
        invite: invite.or_else(|| server.code().map(str::to_owned)),
        server,
        key: key.unwrap_or_else(|| PathBuf::from("./he-cli.key")),
        username,
        addr,
        max_uses,
        expires_in,
        network: NetworkConfig {
            relays: if no_relay {
                Relays::Disabled
            } else if relays.is_empty() {
                Relays::N0
            } else {
                Relays::Custom { urls: relays }
            },
            n0_discovery: !no_dns,
            mdns_discovery: !no_mdns,
        },
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

    let client = Client::bind(&identity, &args.network).await?;

    // A bare EndpointId is resolved by discovery; a link carries its own
    // hints. `--addr` skips both, which is how you test on a LAN with the
    // internet unplugged.
    let addr = match args.addr {
        Some(socket) => {
            EndpointAddr::from_parts(args.server.addr().id, [iroh::TransportAddr::Ip(socket)])
        }
        None => args.server.addr().clone(),
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
        Command::Edit { id, text } => {
            session.edit_message(id, text).await?;
            println!("edited {id}");
        }
        Command::Delete { id } => {
            session.delete_message(id).await?;
            println!("deleted {id}");
        }
        Command::History { channel } => {
            let channel_id = resolve_channel(&session, channel.as_ref())?;
            let messages = session
                .backfill(&channel_id, None, Session::BACKFILL_LIMIT)
                .await?;
            // Newest first on the wire; oldest first on a terminal, because
            // that is the direction a conversation reads.
            for message in messages.iter().rev() {
                println!("{}  {}", message.id, describe(message));
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
            // The code alone is half an invite. The other half is where to
            // send it, and a link carries both (PLAN §5).
            println!("{code}");
            println!(
                "{}",
                InviteLink::new(args.server.addr().clone(), Some(code))
            );
        }
        Command::Members => {
            for member in &session.ready().members {
                let mut tags = Vec::new();
                if member.is_owner {
                    tags.push("owner");
                }
                if member.banned {
                    tags.push("banned");
                }
                let tags = if tags.is_empty() {
                    String::new()
                } else {
                    format!("  ({})", tags.join(", "))
                };
                println!("{}  {}{}", member.id, member.username, tags);
            }
        }
        Command::Devices { user_id } => {
            for device in session.devices(user_id.as_deref()).await? {
                let state = match (device.revoked_at, device.current) {
                    (Some(_), _) => "revoked",
                    (None, true) => "this one",
                    (None, false) => "active",
                };
                println!(
                    "{}  {:<8}  {}",
                    device.endpoint_id,
                    state,
                    device.label.as_deref().unwrap_or("-")
                );
            }
        }
        Command::RevokeDevice {
            user_id,
            endpoint_id,
        } => {
            session.revoke_device(user_id, endpoint_id).await?;
            println!("revoked {endpoint_id}");
        }
        Command::Kick { user_id } => {
            session.kick(user_id).await?;
            println!("kicked {user_id}");
        }
        Command::SetBanned { user_id, banned } => {
            session.set_banned(user_id, *banned).await?;
            println!("{} {user_id}", if *banned { "banned" } else { "un-banned" });
        }
        Command::Invites => {
            for invite in session.invites().await? {
                let uses = match invite.max_uses {
                    Some(max) => format!("{}/{max}", invite.uses),
                    None => format!("{}/∞", invite.uses),
                };
                println!("{}  {uses}", invite.code);
            }
        }
        Command::RevokeInvite { code } => {
            session.revoke_invite(code).await?;
            println!("revoked {code}");
        }
        Command::ChannelAdd { name } => {
            let channel = session.create_channel(name, None).await?;
            println!("{}  {}", channel.id, channel.name);
        }
        Command::ChannelRemove { channel } => {
            let channel_id = resolve_channel(&session, Some(channel))?;
            session.delete_channel(&channel_id).await?;
            println!("deleted {channel_id}");
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

/// One line for a message, with a tombstone where a deleted one used to be.
fn describe(message: &he_proto::Message) -> String {
    if message.deleted_at.is_some() {
        return format!("{:>12}  <deleted>", message.author_name);
    }
    format!("{:>12}  {}", message.author_name, message.content)
}

async fn watch(session: &mut Session) {
    while let Some(frame) = session.next_event().await {
        match frame {
            ServerFrame::Message { message, nonce } => {
                let mine = if nonce.is_some() { " (mine)" } else { "" };
                println!("{}  {}{}", message.id, describe(&message), mine);
            }
            ServerFrame::Edited { message } => {
                println!("{}  {} (edited)", message.id, describe(&message));
            }
            ServerFrame::Deleted { id, channel_id, .. } => {
                println!("{id}  deleted in {channel_id}");
            }
            ServerFrame::Presence { user_id, online } => {
                let state = if online { "online" } else { "offline" };
                println!("{user_id} is {state}");
            }
            ServerFrame::Typing { username, .. } => {
                println!("{username} is typing…");
            }
            ServerFrame::Channels { channels } => {
                let names: Vec<&str> = channels.iter().map(|c| c.name.as_str()).collect();
                println!("channels: {}", names.join(", "));
            }
            ServerFrame::Members { members } => {
                let names: Vec<&str> = members.iter().map(|m| m.username.as_str()).collect();
                println!("members: {}", names.join(", "));
            }
            ServerFrame::Revoked => {
                eprintln!("this device was revoked; not reconnecting");
                return;
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
