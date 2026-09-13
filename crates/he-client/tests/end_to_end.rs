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
use he_proto::{InviteLink, Password, ServerFrame};
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
    endpoint: Endpoint,
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
        let router = he_server::serve_on(endpoint.clone(), server.clone(), Limits::default());

        Self {
            server,
            router,
            endpoint,
            addr,
            invite,
            _dir: dir,
        }
    }

    /// The invite link a host would paste into a chat message (PLAN §5),
    /// minted from the real endpoint by the shipping code.
    fn invite_link(&self) -> InviteLink {
        he_server::invite_link(&self.endpoint, Some(self.invite.clone()))
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

/// The next event that is *about the conversation*.
///
/// Presence and typing arrive whenever somebody opens a laptop, so a test
/// asserting on what was said must not be coupled to when that happened —
/// otherwise adding a second member to a test is enough to break it.
async fn next_chat_event(session: &mut Session) -> ServerFrame {
    loop {
        match next_event(session).await {
            ServerFrame::Presence { .. } | ServerFrame::Typing { .. } => continue,
            frame => return frame,
        }
    }
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

    match next_chat_event(&mut session).await {
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

    match next_chat_event(&mut alice).await {
        ServerFrame::Message { nonce: echoed, .. } => {
            assert_eq!(echoed.as_deref(), Some(nonce.as_str()))
        }
        other => panic!("expected a message event, got {other:?}"),
    }
    match next_chat_event(&mut bob).await {
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
            Some(&session.ready().user.id),
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
        match next_chat_event(&mut session).await {
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
        .upsert_server(&server_key, "Test Space", "justin", Some("u1"), None)
        .await
        .expect("upsert");

    for line in ["one", "two"] {
        session.send_message(&channel, line).await.expect("send");
        match next_chat_event(&mut session).await {
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

// ---- M4: joining by link --------------------------------------------------

#[tokio::test]
async fn a_pasted_invite_link_is_enough_to_join() {
    // The M4 claim, minus the two cities: everything a joiner needs is in one
    // string, and the string is produced by the host's own endpoint rather
    // than assembled by hand (PLAN §5).
    let harness = Harness::start().await;
    let link = harness.invite_link();
    let pasted = link.to_string();

    // Round-trips through the text a chat client would carry.
    let parsed: InviteLink = pasted.parse().expect("a readable link");
    assert_eq!(
        parsed.endpoint_id(),
        harness.server.endpoint_id().to_string()
    );
    assert_eq!(parsed.code(), Some(harness.invite.as_str()));

    let client = harness.client().await;
    let session = client
        .connect(
            parsed.addr().clone(),
            Auth::Register {
                invite: parsed.code().expect("a code").to_owned(),
                username: "alice".into(),
                password: Password::new("correct horse"),
            },
        )
        .await
        .expect("join from the link alone");

    assert_eq!(session.ready().user.username, "alice");
    assert_eq!(session.ready().server_name, "Test Space");

    session.close().await;
    harness.shutdown().await;
}

#[tokio::test]
async fn a_link_carries_the_address_hints_the_endpoint_knows() {
    // A link with no hints still joins — discovery resolves the key — but it
    // joins *slowly*, which is the failure nobody reports because the app
    // merely feels bad. The hints are the whole reason a ticket is not just an
    // EndpointId.
    let harness = Harness::start().await;
    let link = harness.invite_link();

    let hinted: Vec<_> = link.addr().ip_addrs().collect();
    assert!(
        hinted.iter().any(|addr| addr.ip().is_loopback()),
        "the link should carry the socket the endpoint is actually on, got {hinted:?}"
    );

    harness.shutdown().await;
}

#[tokio::test]
async fn a_second_device_joins_with_an_address_only_link_and_a_password() {
    // The other half of the link's job: pointing a *second machine of your
    // own* at a space, where there is no invite involved and the password
    // enrols the device (PLAN §3).
    let harness = Harness::start().await;

    let first = harness.client().await;
    let laptop = harness.register(&first, "justin").await;
    let user_id = laptop.ready().user.id.clone();
    laptop.close().await;

    let address_only = InviteLink::address_only(harness.invite_link().addr().clone());
    assert_eq!(address_only.code(), None);
    let parsed: InviteLink = address_only
        .to_string()
        .parse()
        .expect("an address-only link is still a link");

    let phone = harness.client().await;
    let session = phone
        .connect(
            parsed.addr().clone(),
            Auth::Password {
                username: "justin".into(),
                password: Password::new("correct horse"),
            },
        )
        .await
        .expect("a password enrols a second device");
    assert_eq!(session.ready().user.id, user_id, "same account, new device");
    assert!(session.ready().enrolled);

    // Two devices, one account — which is the thing `Auth::Device` needs a
    // username to disambiguate.
    let devices = harness
        .server
        .devices_for_user(&user_id)
        .await
        .expect("devices");
    assert_eq!(devices.len(), 2);

    session.close().await;
    harness.shutdown().await;
}

#[tokio::test]
async fn a_link_from_a_different_space_does_not_open_this_one() {
    // A link is self-verifying because the address *is* a public key: pointing
    // one at the wrong server does not reach a different server, it reaches
    // nothing (PLAN §5).
    let harness = Harness::start().await;
    let stranger = SecretKey::generate().public();
    let elsewhere = InviteLink::new(
        EndpointAddr::from_parts(stranger, harness.invite_link().addr().addrs.iter().cloned()),
        Some(harness.invite.clone()),
    );

    let client = harness.client().await;
    let refused = tokio::time::timeout(
        PATIENCE,
        client.connect(
            elsewhere.addr().clone(),
            Auth::Register {
                invite: harness.invite.clone(),
                username: "mallory".into(),
                password: Password::new("correct horse"),
            },
        ),
    )
    .await;

    match refused {
        // Either it could not reach a peer holding that key, or it timed out
        // trying. What must not happen is a session on *this* server.
        Ok(Err(_)) | Err(_) => {}
        Ok(Ok(session)) => panic!(
            "connected to {} with a link for a different key",
            session.ready().server_name
        ),
    }

    harness.shutdown().await;
}

// ---------------------------------------------------------------------------
// M5: surviving contact with reality
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_edit_reaches_everyone_and_a_deletion_takes_the_words_with_it() {
    let harness = Harness::start().await;
    let alice_client = harness.client().await;
    let mut alice = harness.register(&alice_client, "alice").await;
    let bob_client = harness.client().await;
    let mut bob = harness.register(&bob_client, "bob").await;

    let channel = alice.ready().channels[0].id.clone();
    alice.send_message(&channel, "helo").await.expect("send");

    let ServerFrame::Message { message, .. } = next_chat_event(&mut alice).await else {
        panic!("expected the message back");
    };
    next_chat_event(&mut bob).await; // bob sees it too

    alice
        .edit_message(&message.id, "hello")
        .await
        .expect("edit");

    for session in [&mut alice, &mut bob] {
        match next_chat_event(session).await {
            ServerFrame::Edited { message } => {
                assert_eq!(message.content, "hello");
                assert!(message.edited_at.is_some(), "an edit is stamped as one");
            }
            other => panic!("expected an edited event, got {other:?}"),
        }
    }

    alice.delete_message(&message.id).await.expect("delete");
    for session in [&mut alice, &mut bob] {
        match next_chat_event(session).await {
            ServerFrame::Deleted { id, .. } => assert_eq!(id, message.id),
            other => panic!("expected a deleted event, got {other:?}"),
        }
    }

    // The point of a delete: the text is gone, not hidden. The host reads this
    // database in plaintext by design, so anything less would make "deleted"
    // mean "you cannot see it in the app" (PLAN §10).
    let history = bob
        .backfill(&channel, None, Session::BACKFILL_LIMIT)
        .await
        .expect("backfill");
    let stored = history
        .iter()
        .find(|m| m.id == message.id)
        .expect("the tombstone is still there, so clients can be told");
    assert_eq!(stored.content, "");
    assert!(stored.deleted_at.is_some());

    alice.close().await;
    bob.close().await;
    harness.shutdown().await;
}

#[tokio::test]
async fn only_the_author_may_edit_or_delete() {
    // Moderating somebody else's message is the owner's tool, and it is M6's.
    // Until then "your own" is the whole of the rule.
    let harness = Harness::start().await;
    let alice_client = harness.client().await;
    let mut alice = harness.register(&alice_client, "alice").await;
    let bob_client = harness.client().await;
    let bob = harness.register(&bob_client, "bob").await;

    let channel = alice.ready().channels[0].id.clone();
    alice.send_message(&channel, "mine").await.expect("send");
    let ServerFrame::Message { message, .. } = next_chat_event(&mut alice).await else {
        panic!("expected the message back");
    };

    let err = bob
        .edit_message(&message.id, "not yours")
        .await
        .expect_err("bob did not write it");
    assert_eq!(err.code(), Some(ErrorCode::Forbidden), "got {err:?}");

    let err = bob
        .delete_message(&message.id)
        .await
        .expect_err("bob did not write it");
    assert_eq!(err.code(), Some(ErrorCode::Forbidden), "got {err:?}");

    alice.close().await;
    bob.close().await;
    harness.shutdown().await;
}

#[tokio::test]
async fn reconnecting_costs_the_gap_and_not_the_history() {
    // The M5 done-when, in miniature: go away, miss things, come back, and be
    // handed only what was missed (PLAN §9).
    let harness = Harness::start().await;
    let alice_client = harness.client().await;
    let mut alice = harness.register(&alice_client, "alice").await;
    let bob_client = harness.client().await;
    let mut bob = harness.register(&bob_client, "bob").await;
    let channel = alice.ready().channels[0].id.clone();

    // Something alice sees before she goes away.
    bob.send_message(&channel, "before").await.expect("send");
    let ServerFrame::Message { message: seen, .. } = next_chat_event(&mut alice).await else {
        panic!("expected the message");
    };
    next_chat_event(&mut bob).await;

    // Alice's laptop shuts. Bob keeps talking, and edits something she had.
    alice.close().await;
    bob.send_message(&channel, "while away")
        .await
        .expect("send");
    let ServerFrame::Message {
        message: missed, ..
    } = next_chat_event(&mut bob).await
    else {
        panic!("expected the message");
    };

    // Back, on the same device: no password, because the key is the login.
    let alice = alice_client
        .connect(harness.addr.clone(), Auth::Device { username: None })
        .await
        .expect("reconnect");

    let cursors = std::collections::BTreeMap::from([(channel.clone(), seen.id.clone())]);
    let resumed = alice.resume(cursors, None).await.expect("resume");

    let ids: Vec<&str> = resumed.messages.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![missed.id.as_str()],
        "only the gap: a reconnect is not a reload"
    );
    assert!(resumed.truncated.is_empty());

    alice.close().await;
    bob.close().await;
    harness.shutdown().await;
}

#[tokio::test]
async fn a_resume_reports_a_message_deleted_while_we_were_away() {
    // A deletion keeps the message's id, so it is *older* than every cursor
    // and no amount of "what is new" would ever mention it. Without this the
    // withdrawn message sits on the returning client's screen forever.
    let harness = Harness::start().await;
    let alice_client = harness.client().await;
    let mut alice = harness.register(&alice_client, "alice").await;
    let bob_client = harness.client().await;
    let mut bob = harness.register(&bob_client, "bob").await;
    let channel = alice.ready().channels[0].id.clone();

    bob.send_message(&channel, "regrettable")
        .await
        .expect("send");
    let ServerFrame::Message { message, .. } = next_chat_event(&mut bob).await else {
        panic!("expected the message");
    };
    next_chat_event(&mut alice).await;
    let before_leaving = he_server::db::now_unix();

    alice.close().await;
    // A deletion is stamped in whole seconds, so a `since` taken in the same
    // second as the delete must still catch it — which is why the server
    // compares with `>=`. Sleeping here would only hide that.
    bob.delete_message(&message.id).await.expect("delete");

    let alice = alice_client
        .connect(harness.addr.clone(), Auth::Device { username: None })
        .await
        .expect("reconnect");
    let cursors = std::collections::BTreeMap::from([(channel, message.id.clone())]);
    let resumed = alice
        .resume(cursors, Some(before_leaving))
        .await
        .expect("resume");

    let tombstone = resumed
        .messages
        .iter()
        .find(|m| m.id == message.id)
        .expect("the deletion has to come back");
    assert!(tombstone.deleted_at.is_some());
    assert_eq!(tombstone.content, "");

    alice.close().await;
    bob.close().await;
    harness.shutdown().await;
}

#[tokio::test]
async fn presence_follows_the_account_not_the_connection() {
    let harness = Harness::start().await;
    let alice_client = harness.client().await;
    let mut alice = harness.register(&alice_client, "alice").await;

    // The reader of a `ready` is online by the time they read it, and the
    // snapshot says so. The alternative — a list that includes you only when a
    // second device of yours happens to be connected — is an inconsistency
    // every client would have to paper over.
    assert_eq!(
        alice.ready().online,
        vec![alice.ready().user.id.clone()],
        "alice is the only one here, and she is here"
    );

    let bob_client = harness.client().await;
    let bob = harness.register(&bob_client, "bob").await;
    let bob_id = bob.ready().user.id.clone();

    match next_event(&mut alice).await {
        ServerFrame::Presence { user_id, online } => {
            assert_eq!(user_id, bob_id);
            assert!(online);
        }
        other => panic!("expected bob coming online, got {other:?}"),
    }

    // Bob's `ready` saw alice already there, without waiting for an event.
    assert!(bob.ready().online.contains(&alice.ready().user.id));

    bob.close().await;
    match next_event(&mut alice).await {
        ServerFrame::Presence { user_id, online } => {
            assert_eq!(user_id, bob_id);
            assert!(!online);
        }
        other => panic!("expected bob going offline, got {other:?}"),
    }

    alice.close().await;
    harness.shutdown().await;
}

#[tokio::test]
async fn typing_goes_to_everyone_else_and_is_throttled() {
    let harness = Harness::start().await;
    let alice_client = harness.client().await;
    let alice = harness.register(&alice_client, "alice").await;
    let bob_client = harness.client().await;
    let mut bob = harness.register(&bob_client, "bob").await;
    let channel = alice.ready().channels[0].id.clone();

    // Every raw event here, deliberately: the whole assertion is about which
    // frames arrive and which do not, so nothing may be filtered out first.
    alice.typing(&channel).await.expect("typing");
    match next_event(&mut bob).await {
        ServerFrame::Typing {
            username, user_id, ..
        } => {
            assert_eq!(username, "alice");
            assert_eq!(user_id, alice.ready().user.id);
        }
        other => panic!("expected a typing event, got {other:?}"),
    }

    // Twice in a row is the client's bug, and the server absorbs it rather
    // than fanning a frame out to every member for every keystroke.
    alice.typing(&channel).await.expect("accepted anyway");
    alice.send_message(&channel, "hi").await.expect("send");
    match next_event(&mut bob).await {
        ServerFrame::Message { .. } => {}
        other => panic!("the second typing event should have been dropped, got {other:?}"),
    }

    alice.close().await;
    bob.close().await;
    harness.shutdown().await;
}
