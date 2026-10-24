//! M6's "done when" (`docs/PLAN.md`), automated on loopback:
//!
//! **a group with an anchor and every member's app closed still admits a
//! new member at 4am** — and the bridge half, short of a real NIP-29 client:
//! a WebSocket client, speaking NIP-01 and NIP-42 as Chachi or 0xchat do,
//! reads the group through the anchor's `ws://` as a member, sees NIP-29
//! metadata signed by the anchor, and a stranger reads nothing. Reading it
//! with Chachi itself is `docs/TESTING.md` §5.

// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{net::SocketAddr, path::Path, time::Duration};

use futures_util::{SinkExt, StreamExt};
use hit_anchor::{nip29, ws};
use hit_event::{MemoryStore, Person, group};
use hit_node::{Config, Node, Relays};
use iroh::{EndpointAddr, TransportAddr};
use nostr::prelude::{
    ClientMessage, Event, EventBuilder, Filter, Kind, RelayMessage, SingleLetterTag,
    SubscriptionId, Tag,
};
use tokio_tungstenite::tungstenite::Message;

fn same(p: &Person) -> Person {
    Person::from_nsec(&p.reveal_nsec_for_backup()).unwrap()
}

async fn start(dir: &Path, p: &Person) -> Node {
    Node::start(
        same(p),
        Config {
            data_dir: dir.to_path_buf(),
            relays: Relays::Disabled,
            port: None,
        },
    )
    .await
    .unwrap()
}

fn local(n: &Node) -> SocketAddr {
    let port = n
        .endpoint()
        .bound_sockets()
        .into_iter()
        .find(SocketAddr::is_ipv4)
        .unwrap()
        .port();
    ([127, 0, 0, 1], port).into()
}

fn addr(n: &Node) -> EndpointAddr {
    EndpointAddr::from_parts(n.id(), [TransportAddr::Ip(local(n))])
}

async fn texts(n: &Node, g: &nostr::prelude::PublicKey) -> Vec<String> {
    n.messages(g, None)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.content)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_group_with_an_anchor_admits_a_new_member_while_every_app_is_closed() {
    let dirs: Vec<_> = (0..4).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, bob, anchor, dave) = (
        Person::generate(),
        Person::generate(),
        Person::generate(),
        Person::generate(),
    );
    let a = start(dirs[0].path(), &alice).await;
    let b = start(dirs[1].path(), &bob).await;
    let n = start(dirs[2].path(), &anchor).await;
    for (x, y) in [(&a, &b), (&a, &n), (&b, &n)] {
        x.add_peer(addr(y));
        y.add_peer(addr(x));
    }
    let g = a
        .create_group("Night owls", &MemoryStore::default())
        .await
        .unwrap();
    let link = a.invite(&g, None, Some(local(&a))).await.unwrap();
    // The anchor pins the group: a member, and says it is an anchor.
    n.join(&link).await.unwrap();
    n.set_anchor_profile("the group's anchor").await.unwrap();
    b.join(&link).await.unwrap();
    a.say(&g, "general", "said before bedtime").await.unwrap();
    for x in [&a, &b] {
        x.sync_with(addr(&n)).await.unwrap();
    }
    assert_eq!(a.anchors(&g).await.unwrap(), vec![anchor.public_key()]);

    // An invite made now names the anchor first.
    let late = a
        .invite(&g, Some(Duration::from_secs(86_400)), Some(local(&a)))
        .await
        .unwrap();
    assert_eq!(
        late.peers[0].0,
        n.id(),
        "the anchor is not first in the link"
    );
    n.sync_with(addr(&a)).await.unwrap();

    // 4am: every member's app is closed. Only the anchor runs.
    let a_dir = dirs[0].path().to_path_buf();
    a.shutdown().await;
    b.shutdown().await;
    let anchor_loop = n.spawn_sync_loop(Duration::from_millis(500));

    // Dave joins from the link. His app finds the anchor by its id — here
    // by its loopback address, which discovery would give it.
    let d = start(dirs[3].path(), &dave).await;
    d.add_peer(addr(&n));
    let joined = d.join(&late).await.unwrap();
    assert!(joined.fetched > 0, "{joined}");
    let s = d.group(&g).unwrap();
    assert!(s.is_member(&dave.public_key()));
    assert!(
        texts(&d, &g)
            .await
            .contains(&"said before bedtime".to_owned())
    );
    d.say(&g, "general", "dave, at 4am").await.unwrap();
    d.sync_with(addr(&n)).await.unwrap();

    // Morning: Alice opens the app and finds Dave, through the anchor.
    let a = start(&a_dir, &alice).await;
    a.add_peer(addr(&n));
    a.sync_with(addr(&n)).await.unwrap();
    assert!(a.group(&g).unwrap().is_member(&dave.public_key()));
    assert!(texts(&a, &g).await.contains(&"dave, at 4am".to_owned()));

    anchor_loop.abort();
    for x in [a, d, n] {
        x.shutdown().await;
    }
}

// --- the ws:// bridge, as a NIP-29 client speaks to it --------------------

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next_relay(socket: &mut Socket) -> RelayMessage<'static> {
    loop {
        let msg = tokio::time::timeout(Duration::from_secs(10), socket.next())
            .await
            .expect("the bridge said nothing")
            .unwrap()
            .unwrap();
        if let Message::Text(t) = msg {
            return RelayMessage::from_json(t.as_str()).unwrap();
        }
    }
}

async fn say(socket: &mut Socket, msg: ClientMessage<'_>) {
    socket
        .send(Message::Text(msg.as_json().into()))
        .await
        .unwrap();
}

/// REQ, then every EVENT up to EOSE (or a CLOSED, as `Err`).
async fn req(socket: &mut Socket, id: &str, filter: Filter) -> Result<Vec<Event>, String> {
    say(
        socket,
        ClientMessage::req(SubscriptionId::new(id), vec![filter]),
    )
    .await;
    let mut out = Vec::new();
    loop {
        match next_relay(socket).await {
            RelayMessage::Event { event, .. } => out.push(event.into_owned()),
            RelayMessage::EndOfStoredEvents(_) => return Ok(out),
            RelayMessage::Closed { message, .. } => return Err(message.into_owned()),
            _ => {}
        }
    }
}

async fn connect(url: &str) -> (Socket, String) {
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let RelayMessage::Auth { challenge } = next_relay(&mut socket).await else {
        panic!("no AUTH challenge first");
    };
    (socket, challenge.into_owned())
}

async fn auth(socket: &mut Socket, who: &Person, url: &str, challenge: &str) -> bool {
    let ev = who
        .sign(
            EventBuilder::new(Kind::Authentication, "")
                .tag(Tag::custom("relay", [url]))
                .tag(Tag::custom("challenge", [challenge])),
        )
        .unwrap();
    say(socket, ClientMessage::auth(ev)).await;
    matches!(
        next_relay(socket).await,
        RelayMessage::Ok { status: true, .. }
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_nip29_client_reads_the_group_through_the_anchor_and_a_stranger_does_not() {
    let dirs: Vec<_> = (0..2).map(|_| tempfile::tempdir().unwrap()).collect();
    let (alice, anchor, eve) = (Person::generate(), Person::generate(), Person::generate());
    let a = start(dirs[0].path(), &alice).await;
    let n = start(dirs[1].path(), &anchor).await;
    a.add_peer(addr(&n));
    n.add_peer(addr(&a));
    let g = a
        .create_group("Bridged", &MemoryStore::default())
        .await
        .unwrap();
    n.join(&a.invite(&g, None, Some(local(&a))).await.unwrap())
        .await
        .unwrap();
    a.set_name("Alice").await.unwrap();
    a.say(&g, "general", "readable from Chachi").await.unwrap();
    n.sync_with(addr(&a)).await.unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("ws://127.0.0.1:{port}");
    let bridge = ws::Bridge::new(
        n.clone(),
        same(&anchor),
        "test anchor".into(),
        Some(url.clone()),
    );
    let server = tokio::spawn(ws::serve_on(bridge, listener));

    // NIP-11, on a plain GET.
    let nip11 = {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        s.write_all(b"GET / HTTP/1.1\r\nHost: x\r\nAccept: application/nostr+json\r\nConnection: close\r\n\r\n")
            .await
            .unwrap();
        let mut body = String::new();
        s.read_to_string(&mut body).await.unwrap();
        body
    };
    assert!(nip11.contains("\"supported_nips\":[1,11,29,42,57]"), "{nip11}");
    assert!(nip11.contains(&anchor.public_key().to_hex()));

    let by_group = Filter::new()
        .kind(group::CHAT_MESSAGE)
        .custom_tag(SingleLetterTag::LOWERCASE_H, g.to_hex());

    // Before AUTH: nothing of a private group (M13: a public one reads to
    // anyone — below).
    let (mut socket, challenge) = connect(&url).await;
    let before = req(&mut socket, "early", by_group.clone()).await.unwrap();
    assert!(before.is_empty(), "{before:?}");
    // An AUTH for another relay, or the wrong challenge, is refused.
    assert!(!auth(&mut socket, &alice, "wss://elsewhere.example", &challenge).await);
    assert!(!auth(&mut socket, &alice, &url, "not-the-challenge").await);
    // As Alice — her own key, as in HIT_ENTER — she reads the group.
    assert!(auth(&mut socket, &alice, &url, &challenge).await);
    let msgs = req(&mut socket, "msgs", by_group.clone()).await.unwrap();
    assert!(msgs.iter().any(|e| e.content == "readable from Chachi"));
    // NIP-29 metadata, signed by the anchor; never the group key's own.
    let meta = req(
        &mut socket,
        "meta",
        Filter::new().kinds([nip29::NIP29_METADATA, nip29::NIP29_MEMBERS]),
    )
    .await
    .unwrap();
    assert!(meta.iter().all(|e| e.pubkey == anchor.public_key()));
    let md = meta
        .iter()
        .find(|e| e.kind == nip29::NIP29_METADATA)
        .unwrap();
    assert_eq!(md.tags.identifier(), Some(g.to_hex()));
    assert!(md.tags.iter().any(|t| t.as_slice() == ["name", "Bridged"]));
    let members = meta
        .iter()
        .find(|e| e.kind == nip29::NIP29_MEMBERS)
        .unwrap();
    assert!(
        members
            .tags
            .iter()
            .any(|t| t.as_slice().get(1) == Some(&alice.public_key().to_hex()))
    );
    // Live: a new message reaches the open subscription.
    a.say(&g, "general", "and live").await.unwrap();
    n.sync_with(addr(&a)).await.unwrap();
    let live = loop {
        if let RelayMessage::Event { event, .. } = next_relay(&mut socket).await {
            break event.into_owned();
        }
    };
    assert_eq!(live.content, "and live");
    // A NIP-29 client's own message has no channel: refused, and said why.
    let plain = alice
        .sign(
            EventBuilder::new(group::CHAT_MESSAGE, "from chachi")
                .tag(Tag::custom("h", [g.to_hex()])),
        )
        .unwrap();
    say(&mut socket, ClientMessage::event(plain)).await;
    let refused = loop {
        if let RelayMessage::Ok {
            status, message, ..
        } = next_relay(&mut socket).await
        {
            break (status, message.into_owned());
        }
    };
    assert!(!refused.0 && refused.1.contains("channel"), "{refused:?}");

    // A stranger authenticates fine — and reads nothing of the group.
    let (mut other, challenge) = connect(&url).await;
    assert!(auth(&mut other, &eve, &url, &challenge).await);
    assert!(req(&mut other, "s", by_group.clone()).await.unwrap().is_empty());
    assert!(
        req(&mut other, "m", Filter::new().kind(nip29::NIP29_METADATA))
            .await
            .unwrap()
            .is_empty()
    );

    // M13: Alice makes the group public. A client that never logs in now
    // reads its messages and its NIP-29 metadata, which says "public".
    a.edit(&g, &[group::Edit::Public(true)]).await.unwrap();
    n.sync_with(addr(&a)).await.unwrap();
    let (mut anon, _) = connect(&url).await;
    let read = req(&mut anon, "p", by_group).await.unwrap();
    assert!(read.iter().any(|e| e.content == "readable from Chachi"));
    let meta = req(&mut anon, "pm", Filter::new().kind(nip29::NIP29_METADATA))
        .await
        .unwrap();
    assert!(meta[0].tags.iter().any(|t| t.as_slice() == ["public"]));
    assert!(meta[0].tags.iter().any(|t| t.as_slice() == ["open"]));

    server.abort();
    a.shutdown().await;
    n.shutdown().await;
}
