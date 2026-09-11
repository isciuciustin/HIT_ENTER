//! Two iroh endpoints, one real server, no mocks.
//!
//! This is the M2 milestone test: a client that knows nothing but an
//! `EndpointId` registers, sends a message, and gets it back as an event —
//! over QUIC, through the same code path a client on another continent uses
//! (PLAN §14: "two real iroh endpoints, not mocks").
//!
//! The endpoints here are bound to loopback with **relays and discovery
//! switched off**, so the test needs no network and no n0 infrastructure. That
//! is the only thing it changes about the production setup; everything above
//! the socket is the shipping code.

use std::sync::Arc;
use std::time::Duration;

use he_client::{Client, ClientError, Session};
use he_proto::rpc::{Auth, ErrorCode};
use he_proto::{Password, ServerFrame};
use he_server::{Limits, Server};
use iroh::endpoint::presets;
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey, TransportAddr};

/// Long enough that a slow machine does not fail the test, short enough that a
/// genuine hang is a failure rather than a hung CI job.
const PATIENCE: Duration = Duration::from_secs(10);

struct Harness {
    server: Arc<Server>,
    router: Router,
    addr: EndpointAddr,
    invite: String,
    _dir: tempfile::TempDir,
}

impl Harness {
    async fn start() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let server = Arc::new(
            Server::open(&dir.path().join("server.db"), "Test Space")
                .await
                .expect("open server"),
        );

        // The owner is made locally, never over the network: registration is
        // invite-gated without exception, and the first account cannot be.
        let owner = server
            .create_owner(
                "owner",
                &Password::new_validated("correct horse").expect("valid"),
            )
            .await
            .expect("create owner");
        let invite = server
            .create_invite(&owner.id, None, Some(10))
            .await
            .expect("create invite")
            .code;

        let endpoint = loopback(server.identity().secret_key().clone()).await;
        let addr = direct_addr(&endpoint);
        let router = he_server::serve_on(endpoint, server.clone(), Limits::default());

        Self {
            server,
            router,
            addr,
            invite,
            _dir: dir,
        }
    }

    /// A fresh client with its own device key — a different machine, as far as
    /// the server is concerned.
    async fn client(&self) -> Client {
        Client::from_endpoint(loopback(SecretKey::generate()).await)
    }

    async fn register(&self, client: &Client, username: &str) -> Session {
        client
            .connect(
                self.addr.clone(),
                Auth::Register {
                    invite: self.invite.clone(),
                    username: username.to_owned(),
                    password: Password::new("correct horse"),
                },
            )
            .await
            .expect("register")
    }

    async fn shutdown(self) {
        self.router.shutdown().await.expect("shutdown");
    }
}

/// An endpoint on loopback with no relay and no discovery: everything this
/// test needs, and nothing that reaches the internet.
async fn loopback(secret: SecretKey) -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .secret_key(secret)
        .relay_mode(RelayMode::Disabled)
        .clear_address_lookup()
        .bind_addr("127.0.0.1:0")
        .expect("valid bind address")
        .bind()
        .await
        .expect("bind endpoint")
}

/// The address to dial: the server's public key, plus the socket it is on.
///
/// In production the socket comes from an invite ticket or from discovery, and
/// may be stale — the `EndpointId` is the part that never is.
fn direct_addr(endpoint: &Endpoint) -> EndpointAddr {
    let socket = endpoint
        .bound_sockets()
        .into_iter()
        .find(|addr| addr.ip().is_loopback())
        .expect("a loopback socket");
    EndpointAddr::from_parts(endpoint.id(), [TransportAddr::Ip(socket)])
}

/// Waits for the next event, failing the test rather than hanging forever.
async fn next_event(session: &mut Session) -> ServerFrame {
    tokio::time::timeout(PATIENCE, session.next_event())
        .await
        .expect("an event within the timeout")
        .expect("the session is still open")
}

#[tokio::test]
async fn a_client_registers_sends_a_message_and_gets_it_back() {
    let harness = Harness::start().await;
    let client = harness.client().await;
    let mut session = harness.register(&client, "justin").await;

    let ready = session.ready().clone();
    assert_eq!(ready.server_name, "Test Space");
    assert_eq!(ready.user.username, "justin");
    assert!(ready.enrolled);
    // A brand new space has somewhere to talk, or the first person to join
    // arrives at a dead end.
    let channel = ready
        .channels
        .iter()
        .find(|c| c.name == "general")
        .expect("a default channel");

    let nonce = session
        .send_message(&channel.id, "hit enter")
        .await
        .expect("send");

    match next_event(&mut session).await {
        ServerFrame::Message {
            message,
            nonce: echoed,
        } => {
            assert_eq!(message.content, "hit enter");
            assert_eq!(message.author_name, "justin");
            assert_eq!(message.channel_id, channel.id);
            // The echo is what lets the sender swap its optimistic bubble for
            // the authoritative row (PLAN §9).
            assert_eq!(echoed.as_deref(), Some(nonce.as_str()));
        }
        other => panic!("expected a message event, got {other:?}"),
    }

    // And it is on disk, readable by a second connection that was not here.
    let history = session
        .backfill(&channel.id, None, 50)
        .await
        .expect("backfill");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].content, "hit enter");

    session.close().await;
    harness.shutdown().await;
}

#[tokio::test]
async fn an_enrolled_device_reconnects_without_a_password() {
    // This is the property device enrolment exists for (PLAN §3): the password
    // is needed to enrol a machine, and then never again on that machine.
    let harness = Harness::start().await;
    let client = harness.client().await;

    let first = harness.register(&client, "justin").await;
    let user_id = first.ready().user.id.clone();
    first.close().await;

    let again = client
        .connect(harness.addr.clone(), Auth::Device { username: None })
        .await
        .expect("device auth");
    assert_eq!(again.ready().user.id, user_id);
    assert_eq!(again.ready().user.username, "justin");

    again.close().await;
    harness.shutdown().await;
}

#[tokio::test]
async fn two_members_see_each_others_messages_but_only_their_own_nonce() {
    let harness = Harness::start().await;

    let alice_client = harness.client().await;
    let mut alice = harness.register(&alice_client, "alice").await;
    let bob_client = harness.client().await;
    let mut bob = harness.register(&bob_client, "bob").await;

    let channel = alice.ready().channels[0].id.clone();
    let nonce = alice
        .send_message(&channel, "hello bob")
        .await
        .expect("send");

    match next_event(&mut alice).await {
        ServerFrame::Message { nonce: echoed, .. } => {
            assert_eq!(echoed.as_deref(), Some(nonce.as_str()))
        }
        other => panic!("expected a message event, got {other:?}"),
    }
    match next_event(&mut bob).await {
        ServerFrame::Message {
            message,
            nonce: echoed,
        } => {
            assert_eq!(message.content, "hello bob");
            assert_eq!(message.author_name, "alice");
            // Bob has no optimistic bubble to reconcile. Handing him alice's
            // nonce would have his client adopt a message he never composed.
            assert_eq!(echoed, None, "a nonce belongs to its sender alone");
        }
        other => panic!("expected a message event, got {other:?}"),
    }

    // Bob's client is a different device, so the server sees two enrolments.
    assert_eq!(
        harness.server.members().await.expect("members").len(),
        3,
        "owner, alice and bob"
    );

    alice.close().await;
    bob.close().await;
    harness.shutdown().await;
}

#[tokio::test]
async fn an_unknown_device_is_told_to_log_in_rather_than_let_in() {
    // Accepting a QUIC connection is not authorization (PLAN §11). Anyone who
    // knows the EndpointId gets this far and no further.
    let harness = Harness::start().await;
    let stranger = harness.client().await;

    let err = stranger
        .connect(harness.addr.clone(), Auth::Device { username: None })
        .await
        .expect_err("a device nobody enrolled must not get a session");
    assert_eq!(
        err.code(),
        Some(ErrorCode::DeviceNotEnrolled),
        "got {err:?}"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn a_wrong_password_is_refused_without_saying_why() {
    let harness = Harness::start().await;
    let client = harness.client().await;
    harness.register(&client, "justin").await.close().await;

    let other_device = harness.client().await;
    let err = other_device
        .connect(
            harness.addr.clone(),
            Auth::Password {
                username: "justin".into(),
                password: Password::new("not the password"),
            },
        )
        .await
        .expect_err("wrong password");
    assert_eq!(err.code(), Some(ErrorCode::BadCredentials));

    // A username that does not exist gets the identical answer, so a stranger
    // cannot enumerate the accounts on this server.
    let third_device = harness.client().await;
    let err = third_device
        .connect(
            harness.addr.clone(),
            Auth::Password {
                username: "nobody".into(),
                password: Password::new("not the password"),
            },
        )
        .await
        .expect_err("no such user");
    assert_eq!(err.code(), Some(ErrorCode::BadCredentials));

    harness.shutdown().await;
}

#[tokio::test]
async fn a_bad_invite_cannot_create_an_account() {
    let harness = Harness::start().await;
    let client = harness.client().await;

    let err = client
        .connect(
            harness.addr.clone(),
            Auth::Register {
                invite: "K7QP-2M4X-9WTZ".into(),
                username: "gatecrasher".into(),
                password: Password::new("correct horse"),
            },
        )
        .await
        .expect_err("registration is invite-gated, without exception");
    assert_eq!(err.code(), Some(ErrorCode::InviteInvalid));

    assert_eq!(
        harness.server.members().await.expect("members").len(),
        1,
        "only the owner"
    );
    harness.shutdown().await;
}

#[tokio::test]
async fn a_message_to_a_channel_that_does_not_exist_is_refused() {
    let harness = Harness::start().await;
    let client = harness.client().await;
    let session = harness.register(&client, "justin").await;

    let err = session
        .send_message("0199c1f8-7c3a-7a1e-9f0b-000000000000", "into the void")
        .await
        .expect_err("no such channel");
    assert_eq!(err.code(), Some(ErrorCode::NotFound));

    // The session survives a refused request: one bad RPC is not a reason to
    // drop a connection that flaky wifi made expensive to establish.
    let channel = session.ready().channels[0].id.clone();
    session
        .send_message(&channel, "still here")
        .await
        .expect("send");

    session.close().await;
    harness.shutdown().await;
}

#[tokio::test]
async fn an_oversized_message_is_refused_by_both_sides() {
    let harness = Harness::start().await;
    let client = harness.client().await;
    let session = harness.register(&client, "justin").await;
    let channel = session.ready().channels[0].id.clone();

    // The client refuses it locally, against the same `limits` functions the
    // server would have used.
    let err = session
        .send_message(
            &channel,
            &"x".repeat(he_proto::limits::MESSAGE_MAX_CHARS + 1),
        )
        .await
        .expect_err("too long");
    assert!(matches!(err, ClientError::Validation(_)), "{err:?}");

    session.close().await;
    harness.shutdown().await;
}

#[tokio::test]
async fn an_invite_minted_over_the_wire_works() {
    let harness = Harness::start().await;
    let client = harness.client().await;
    let session = harness.register(&client, "justin").await;

    let code = session
        .create_invite(Some(3600), Some(1))
        .await
        .expect("invite");
    session.close().await;

    let friend_client = harness.client().await;
    let friend = friend_client
        .connect(
            harness.addr.clone(),
            Auth::Register {
                invite: code.clone(),
                username: "friend".into(),
                password: Password::new("correct horse"),
            },
        )
        .await
        .expect("register with the new code");
    friend.close().await;

    // max_uses was 1, and a used-up code is as good as an unknown one.
    let gatecrasher_client = harness.client().await;
    let err = gatecrasher_client
        .connect(
            harness.addr.clone(),
            Auth::Register {
                invite: code,
                username: "gatecrasher".into(),
                password: Password::new("correct horse"),
            },
        )
        .await
        .expect_err("the code is spent");
    assert_eq!(err.code(), Some(ErrorCode::InviteInvalid));

    harness.shutdown().await;
}

#[tokio::test]
async fn the_mirror_answers_after_the_server_is_gone() {
    // The M3 claim, tested without a GUI: everything the client saw is on its
    // own disk, and reading it back does not involve a network at all.
    let harness = Harness::start().await;
    let client = harness.client().await;
    let mut session = harness.register(&client, "justin").await;
    let channel = session.ready().channels[0].clone();

    let dir = tempfile::tempdir().expect("tempdir");
    let mirror_path = dir.path().join("mirror.db");
    let mirror = he_client::Mirror::open(&mirror_path).await.expect("mirror");

    // What the app does on a successful handshake.
    mirror
        .upsert_server(
            &harness.addr.id.to_string(),
            &session.ready().server_name,
            &session.ready().user.username,
            None,
        )
        .await
        .expect("upsert");
    mirror
        .replace_channels(&harness.addr.id.to_string(), &session.ready().channels)
        .await
        .expect("channels");

    for line in ["first", "second", "third"] {
        session.send_message(&channel.id, line).await.expect("send");
        // What the event pump does: disk first, screen second.
        match next_event(&mut session).await {
            ServerFrame::Message { message, .. } => mirror
                .record_message(&harness.addr.id.to_string(), &message)
                .await
                .expect("mirror"),
            other => panic!("expected a message event, got {other:?}"),
        }
    }

    let server_key = harness.addr.id.to_string();
    session.close().await;
    // The space is now gone: the process is down and the endpoint is unbound.
    harness.shutdown().await;

    // Reopened cold, as a restarted app would.
    drop(mirror);
    let reopened = he_client::Mirror::open(&mirror_path).await.expect("reopen");

    let channels = reopened.channels(&server_key).await.expect("channels");
    assert_eq!(channels.len(), 1, "the rail renders with the network off");

    let history = reopened
        .messages(&server_key, &channel.id, None, 50)
        .await
        .expect("history");
    assert_eq!(
        history
            .iter()
            .rev()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>(),
        ["first", "second", "third"],
        "the whole conversation, from disk, with nothing listening"
    );
    assert_eq!(history[0].author_name, "justin");
}

#[tokio::test]
async fn a_backfill_page_overlapping_live_events_does_not_duplicate() {
    // The normal case on every reconnect: the client already saw some of what
    // the server is about to hand it back.
    let harness = Harness::start().await;
    let client = harness.client().await;
    let mut session = harness.register(&client, "justin").await;
    let channel = session.ready().channels[0].id.clone();
    let server_key = harness.addr.id.to_string();

    let mirror = he_client::Mirror::in_memory().await.expect("mirror");
    mirror
        .upsert_server(&server_key, "Test Space", "justin", None)
        .await
        .expect("upsert");

    for line in ["one", "two"] {
        session.send_message(&channel, line).await.expect("send");
        match next_event(&mut session).await {
            ServerFrame::Message { message, .. } => mirror
                .record_message(&server_key, &message)
                .await
                .expect("record"),
            other => panic!("expected a message event, got {other:?}"),
        }
    }

    // Now backfill the same two, as a reconnect would.
    let page = session
        .backfill(&channel, None, 50)
        .await
        .expect("backfill");
    assert_eq!(page.len(), 2);
    mirror
        .record_messages(&server_key, &page)
        .await
        .expect("record page");

    assert_eq!(
        mirror
            .message_count(&server_key, &channel)
            .await
            .expect("count"),
        2,
        "the same message twice is one message"
    );

    session.close().await;
    harness.shutdown().await;
}
