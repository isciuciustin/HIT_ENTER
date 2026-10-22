//! S1 — Nostr over iroh. See ../README.md.

mod client;
mod frame;
mod node;
mod relay;
mod store;

use anyhow::Result;
use clap::{Parser, Subcommand};
use iroh::EndpointId;
use nostr::event::FinalizeEvent;
use nostr::prelude::{EventBuilder, Filter, Keys, Kind};
use std::path::PathBuf;
use store::Store;

#[derive(Parser)]
#[command(name = "nostr-iroh")]
struct Cli {
    /// Directory for this node's iroh key and event store.
    #[arg(long, global = true, default_value = "./data")]
    data: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a relay on hit/nostr/1 and print the node id.
    Serve,
    /// Sign N text notes with a throwaway key and put them in the local store.
    Publish {
        text: String,
        #[arg(long, default_value_t = 1)]
        count: u32,
    },
    /// Send one EVENT to a peer and print its verdict. --tamper changes the
    /// content after signing, so the id/signature no longer match.
    Send {
        #[arg(long)]
        peer: EndpointId,
        text: String,
        #[arg(long)]
        tamper: bool,
    },
    /// Print events: from a peer over iroh with --peer, else from the local store.
    Req {
        #[arg(long)]
        peer: Option<EndpointId>,
        #[arg(long)]
        kind: Option<u16>,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Negentropy-sync with a peer, both directions.
    Sync {
        #[arg(long)]
        peer: EndpointId,
    },
    /// Number of events in the local store.
    Count,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "nostr_iroh=info".into()))
        .init();
    let cli = Cli::parse();
    let store = Store::open(&cli.data).await?;

    match cli.cmd {
        Cmd::Serve => {
            let ep = node::bind(&cli.data, true).await?;
            ep.online().await;
            println!("node id: {}", ep.id());
            println!("events:  {}", store.count().await?);
            while let Some(incoming) = ep.accept().await {
                let store = store.clone();
                tokio::spawn(async move {
                    match incoming.await {
                        Ok(conn) => {
                            if let Err(e) = relay::serve_connection(conn, store).await {
                                tracing::warn!("session ended: {e:#}");
                            }
                        }
                        Err(e) => tracing::warn!("handshake failed: {e:#}"),
                    }
                });
            }
        }
        Cmd::Publish { text, count } => {
            let keys = Keys::generate();
            println!("author: {}", keys.public_key());
            for i in 0..count {
                let body = if count == 1 { text.clone() } else { format!("{text} #{i}") };
                let event = EventBuilder::new(Kind::TextNote, body).finalize(&keys)?;
                store.insert(&event).await?;
            }
            println!("stored {count}; local total {}", store.count().await?);
        }
        Cmd::Send { peer, text, tamper } => {
            let keys = Keys::generate();
            let mut event = EventBuilder::new(Kind::TextNote, text).finalize(&keys)?;
            if tamper {
                event.content.push_str(" (edited after signing)");
            }
            let ep = node::bind(&cli.data, false).await?;
            let mut s = client::Session::dial(&ep, peer).await?;
            let (ok, msg) = s.publish(event).await?;
            println!("relay says: ok={ok} {msg:?}");
            s.close().await?;
            ep.close().await;
        }
        Cmd::Req { peer, kind, limit } => {
            let mut filter = Filter::new();
            if let Some(k) = kind {
                filter = filter.kind(Kind::from(k));
            }
            if let Some(l) = limit {
                filter = filter.limit(l);
            }
            let events = match peer {
                None => store.query(&filter).await?,
                Some(peer) => {
                    let ep = node::bind(&cli.data, false).await?;
                    let mut s = client::Session::dial(&ep, peer).await?;
                    let events = s.req(vec![filter]).await?;
                    println!("({} bytes in, {} out)", s.bytes_in, s.bytes_out);
                    s.close().await?;
                    ep.close().await;
                    events
                }
            };
            for e in &events {
                println!("{} {} {:?}", e.created_at.as_secs(), &e.id.to_hex()[..12], e.content);
            }
            println!("{} events", events.len());
        }
        Cmd::Sync { peer } => {
            let ep = node::bind(&cli.data, false).await?;
            let t = std::time::Instant::now();
            let mut s = client::Session::dial(&ep, peer).await?;
            let dial = t.elapsed();
            let stats = s.sync(&store, Filter::new()).await?;
            println!("{stats:#?}");
            println!("dial {dial:?}, total {:?}, local now {}", t.elapsed(), store.count().await?);
            s.close().await?;
            ep.close().await;
        }
        Cmd::Count => println!("{}", store.count().await?),
    }
    Ok(())
}
