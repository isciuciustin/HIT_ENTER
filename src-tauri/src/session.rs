//! Keeping a session alive across everything a network does to it.
//!
//! A connection to a space is not a thing the user established once; it is a
//! thing the app is responsible for *having*. Wifi goes, a hotspot arrives, a
//! laptop suspends on a train and wakes in an office. None of that is an error
//! the user should have to acknowledge with a button.
//!
//! So every connected space has a **supervisor**: one task that owns the
//! session, runs the event pump, and — when the pump returns because the
//! session ended — redials with backoff and catches up. Nothing above it ever
//! sees the gap; the mirror answered the whole time (PLAN §2.1, §6).
//!
//! ## Catching up is not reloading
//!
//! After every successful connection, including the first, the supervisor
//! [`catch_up`]s: it `resume`s from the per-channel cursors in the mirror and
//! then drains the outbox. A reconnect after a dropped packet costs a few
//! hundred bytes, not a re-download of every channel (PLAN §9).

use std::sync::Arc;
use std::time::Duration;

use he_client::{ClientError, ConnectionPath, Session};
use he_proto::ServerFrame;
use he_proto::rpc::{Auth, ErrorCode, Request, Response};
use iroh::EndpointAddr;
use tauri::AppHandle;
use tokio::sync::mpsc;

use crate::events::{self, describe};
use crate::state::App;

/// How long to try one dial before giving up on it and backing off.
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);

/// The first pause after a session drops.
///
/// Short, because the overwhelmingly common cause is a network that came back
/// two seconds ago and nothing has noticed yet.
const BACKOFF_START: Duration = Duration::from_secs(1);

/// The longest pause between attempts.
///
/// A laptop shut in a bag for an hour should not come out of it needing
/// another half hour to notice there is wifi. A minute is long enough to cost
/// nothing and short enough that opening the lid feels instant.
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Starts the supervisor for a session that has just connected.
///
/// Takes the session already handshaked rather than dialling one, because the
/// *first* connection is the one whose failure the user is waiting to hear
/// about — a wrong password has to come back from the command they typed it
/// into, not appear in a log ten seconds later.
pub fn spawn(
    app_handle: AppHandle,
    app: Arc<App>,
    endpoint_id: String,
    session: Session,
    events: mpsc::Receiver<ServerFrame>,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(supervise(app_handle, app, endpoint_id, session, events))
}

async fn supervise(
    app_handle: AppHandle,
    app: Arc<App>,
    endpoint_id: String,
    mut session: Session,
    mut events: mpsc::Receiver<ServerFrame>,
) {
    // Set at the top of every pass: a connection that lasted long enough to
    // catch up has earned a short pause the next time it drops, rather than
    // inheriting whatever backoff it took to establish.
    let mut backoff;

    loop {
        let live = Arc::new(session);
        // Who is here, as of this handshake. Replaced wholesale on every
        // reconnect: merging would keep members who left while we were away.
        let online = live.ready().online.clone();

        // The lists, as of this handshake, likewise wholesale. The window
        // painted from the mirror before this connection existed, and whatever
        // changed while we were away — a member who joined, a channel that
        // went — is in `Ready` and nowhere else. No event will repeat it.
        let ready = live.ready();
        events::apply_channels(
            &app_handle,
            app.mirror(),
            &endpoint_id,
            ready.channels.clone(),
        )
        .await;
        events::apply_members(
            &app_handle,
            app.mirror(),
            &endpoint_id,
            ready.members.clone(),
        )
        .await;

        app.set_live_session(&endpoint_id, live.clone()).await;
        app.seed_online(&endpoint_id, online.clone()).await;
        events::emit_connection(&app_handle, &endpoint_id, describe(live.path()));
        events::emit_presence_sync(&app_handle, &endpoint_id, online);

        // Before the pump, so that what arrives during the catch-up lands
        // *after* the gap it is filling rather than in the middle of it.
        catch_up(&app_handle, &app, &endpoint_id, &live).await;

        backoff = BACKOFF_START;

        events::pump(
            app_handle.clone(),
            Arc::clone(&app),
            live.clone(),
            events,
            endpoint_id.clone(),
        )
        .await;

        // The pump returned, so the session is over. Let go of it before
        // dialling: a command that runs in the gap should see "offline" and
        // write to the outbox, not hand a message to a dead connection.
        app.clear_live_session(&endpoint_id).await;
        events::emit_presence_sync(&app_handle, &endpoint_id, Vec::new());
        live.disconnect();
        drop(live);

        if !app.is_connected(&endpoint_id).await {
            // The user disconnected, or forgot the space. Not a drop.
            break;
        }

        events::emit_connection(&app_handle, &endpoint_id, "connecting");
        tracing::info!(server = %endpoint_id, "session dropped; reconnecting");

        match redial(&app, &endpoint_id, &mut backoff).await {
            Some((next, next_events)) => {
                session = next;
                events = next_events;
            }
            None => break,
        }
    }

    events::emit_connection(&app_handle, &endpoint_id, describe(ConnectionPath::Offline));
    tracing::info!(server = %endpoint_id, "supervisor stopped");
}

/// Dials until it succeeds, the space says never, or the task is aborted.
///
/// Returns `None` only for a refusal there is no point retrying — the two that
/// matter are a revoked device and a device that was never enrolled, both of
/// which need a person, not another attempt.
async fn redial(
    app: &Arc<App>,
    endpoint_id: &str,
    backoff: &mut Duration,
) -> Option<(Session, mpsc::Receiver<ServerFrame>)> {
    loop {
        tokio::time::sleep(*backoff).await;
        *backoff = (*backoff * 2).min(BACKOFF_MAX);

        // Re-read the address every time. A space's relay can change while we
        // are away, and the mirror is where the last one we saw is recorded —
        // the `EndpointId` is the part that never goes stale (PLAN §4).
        let (addr, username) = match address_for(app, endpoint_id).await {
            Some(pair) => pair,
            None => return None, // the space was forgotten while we waited
        };

        let dialing = app.client().connect(addr, Auth::Device { username });
        match tokio::time::timeout(DIAL_TIMEOUT, dialing).await {
            Ok(Ok(mut session)) => {
                let events = session.take_events()?;
                return Some((session, events));
            }
            Ok(Err(ClientError::Refused(err)))
                if matches!(
                    err.code,
                    ErrorCode::DeviceRevoked | ErrorCode::DeviceNotEnrolled
                ) =>
            {
                // Retrying cannot change this answer. PROTOCOL.md §4: stop.
                tracing::warn!(server = %endpoint_id, code = ?err.code, "space refused this device");
                return None;
            }
            Ok(Err(err)) => {
                tracing::debug!(server = %endpoint_id, %err, "reconnect failed");
            }
            Err(_elapsed) => {
                tracing::debug!(server = %endpoint_id, "reconnect timed out");
            }
        }

        if !app.is_connected(endpoint_id).await {
            return None;
        }
    }
}

/// Where to dial a known space, and which account to claim there.
async fn address_for(app: &Arc<App>, endpoint_id: &str) -> Option<(EndpointAddr, Option<String>)> {
    let known = app.mirror().server(endpoint_id).await.ok().flatten()?;
    let id: iroh::EndpointId = endpoint_id.parse().ok()?;

    let mut addr = EndpointAddr::new(id);
    if let Some(relay) = known.relay_url.as_deref()
        && let Ok(url) = relay.parse()
    {
        addr = addr.with_relay_url(url);
    }
    Some((addr, Some(known.username)))
}

/// Closes the gap a disconnection left: what was missed, then what was unsent.
///
/// Both halves are best-effort and neither is fatal. A catch-up that fails has
/// cost nothing that is not still on disk — the cursors have not moved, the
/// outbox has not been cleared — so the next reconnect tries the same thing
/// again.
pub async fn catch_up(
    app_handle: &AppHandle,
    app: &Arc<App>,
    endpoint_id: &str,
    session: &Session,
) {
    if let Err(err) = resume(app_handle, app, endpoint_id, session).await {
        tracing::warn!(server = %endpoint_id, %err, "could not resume");
    }
    if let Err(err) = drain_outbox(app, endpoint_id, session).await {
        tracing::warn!(server = %endpoint_id, %err, "could not drain the outbox");
    }
}

/// Asks for everything missed since this client's cursors.
async fn resume(
    app_handle: &AppHandle,
    app: &Arc<App>,
    endpoint_id: &str,
    session: &Session,
) -> he_client::Result<()> {
    let mirror = app.mirror();
    let cursors = mirror.cursors(endpoint_id).await?;
    if cursors.is_empty() {
        // Nothing mirrored yet, so there is no gap to close — the pane will
        // fill itself from a backfill the moment a channel is opened.
        mirror.mark_resumed(endpoint_id).await?;
        return Ok(());
    }

    let since = mirror.resumed_at(endpoint_id).await?;
    let resumed = session.resume(cursors, since).await?;
    let missed = resumed.messages.len();

    for message in resumed.messages {
        // Disk first, screen second, exactly as for a live event — the whole
        // of `deliver` applies, including the case where what we missed was
        // somebody deleting something we are still showing.
        events::deliver(app_handle, mirror, endpoint_id, message, None).await;
    }

    // A gap too big to carry in one answer. Pull the newest page instead, so
    // the pane is current even though the middle is missing; scrolling back
    // fills the rest through the ordinary backfill path.
    for channel_id in &resumed.truncated {
        tracing::info!(server = %endpoint_id, %channel_id, "gap too large to resume; backfilling");
        if let Ok(page) = session
            .backfill(channel_id, None, Session::BACKFILL_LIMIT)
            .await
        {
            for message in page.into_iter().rev() {
                events::deliver(app_handle, mirror, endpoint_id, message, None).await;
            }
        }
    }

    // Only once everything above is on disk. A mark that ran ahead of the
    // writes would skip whatever a crash in between lost.
    mirror.mark_resumed(endpoint_id).await?;
    if missed > 0 {
        tracing::info!(server = %endpoint_id, missed, "resumed");
    }
    Ok(())
}

/// Sends everything composed while there was nothing to send it down.
///
/// Oldest first, so a conversation typed into a tunnel arrives in the order it
/// was written. Delivery is **at least once**: a message the server stored but
/// whose acknowledgement was lost is still in the outbox and is sent again.
/// The alternative — dropping anything we are unsure about — loses words the
/// user typed, and this way round the failure is visible and recoverable.
async fn drain_outbox(
    app: &Arc<App>,
    endpoint_id: &str,
    session: &Session,
) -> he_client::Result<()> {
    let mirror = app.mirror();
    let pending = mirror.pending(endpoint_id).await?;
    if pending.is_empty() {
        return Ok(());
    }
    tracing::info!(server = %endpoint_id, queued = pending.len(), "draining the outbox");

    for queued in pending {
        let sent = session
            .request(Request::Send {
                channel_id: queued.channel_id.clone(),
                content: queued.content.clone(),
                nonce: queued.nonce.clone(),
            })
            .await;

        match sent {
            Ok(Response::Ok) => {
                // The echo clears it too. Clearing it here as well means a
                // message is not re-sent just because the event that would
                // have retired it was still in flight when the app closed.
                mirror.dequeue(&queued.nonce).await?;
            }
            // The space refused this one — a deleted channel, a message that
            // is now too long for a changed limit. Retrying forever would
            // block every message behind it, so it is dropped from the queue
            // and the failure stays visible in the log.
            Ok(_) | Err(ClientError::Refused(_)) => {
                tracing::warn!(server = %endpoint_id, "a queued message was refused; dropping it");
                mirror.dequeue(&queued.nonce).await?;
            }
            // The connection went away mid-drain. Everything left stays
            // queued, which is exactly what the outbox is for.
            Err(err) => return Err(err),
        }
    }
    Ok(())
}
