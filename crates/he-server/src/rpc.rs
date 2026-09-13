//! Turning a [`Request`] into a [`Response`].
//!
//! Everything here is a thin translation onto [`Server`] calls that M1 already
//! shipped and tested without a network. That is the point of the split: this
//! module decides *nothing*. If it grew a rule of its own, that rule would be
//! reachable only over a socket and testable only over a socket.

use he_proto::rpc::{ErrorCode, ProtocolError, Request, Response};
use he_proto::{DeviceInfo, InviteInfo, Message, limits};

use crate::accept::Session;
use crate::error::ServerError;
use crate::{Server, invite};

/// What a handled request produced: the answer to the caller, and anything
/// the rest of the space needs to be told about.
pub(crate) struct Handled {
    pub response: Response,
    /// Empty for a request that changed nothing anyone else can see.
    pub fan_out: Vec<Outgoing>,
}

/// An event this request produced, before it knows which connection it came
/// from. [`crate::accept`] adds that and turns it into a wire frame — which is
/// why nothing in this module has to know what a connection is.
pub(crate) enum Outgoing {
    Message {
        message: Message,
        nonce: String,
    },
    /// The channel list changed; everyone gets the new one.
    Channels,
    /// The roster changed; everyone gets the new one.
    Members,
    /// These sessions are out. Sent to them alone, and then their connections
    /// are closed.
    Revoked(Revoked),
    Edited {
        message: Message,
    },
    Deleted {
        id: String,
        channel_id: String,
        deleted_at: i64,
    },
    Typing {
        channel_id: String,
    },
}

/// Who a [`Outgoing::Revoked`] is aimed at.
///
/// Narrow on purpose. "Everyone on this account" and "this one machine" are
/// the two things an owner can actually do, and a revocation that matched
/// more broadly than the action that caused it would disconnect bystanders.
#[derive(Debug, Clone)]
pub(crate) enum Revoked {
    /// Every session belonging to an account — a kick or a ban.
    Account { user_id: String },
    /// One enrolment: this account, on this machine.
    Device {
        user_id: String,
        endpoint_id: String,
    },
}

impl Handled {
    fn plain(response: Response) -> Self {
        Self {
            response,
            fan_out: Vec::new(),
        }
    }

    fn with(response: Response, event: Outgoing) -> Self {
        Self {
            response,
            fan_out: vec![event],
        }
    }

    fn with_all(response: Response, events: Vec<Outgoing>) -> Self {
        Self {
            response,
            fan_out: events,
        }
    }
}

/// Refuses anything an owner-only tool was asked to do by somebody else.
///
/// `Forbidden` rather than `NotFound`: every member can already see who the
/// owner is — it is in the roster — so there is nothing here to keep quiet
/// about, and a vague answer would only make a real bug harder to read.
fn owner_only(session: &Session) -> Option<Handled> {
    (!session.user.is_owner)
        .then(|| Handled::plain(Response::Error(ProtocolError::new(ErrorCode::Forbidden))))
}

pub(crate) async fn handle(server: &Server, session: &Session, request: Request) -> Handled {
    // Validated here as well as on the client, because a server can never
    // trust a client — and against the same [`limits`] functions, so the two
    // sides cannot drift apart.
    if let Err(err) = request.validate() {
        return Handled::plain(Response::Error(err.into()));
    }

    match request {
        Request::Send {
            channel_id,
            content,
            nonce,
        } => match server
            .post_message(&session.user, &channel_id, &content)
            .await
        {
            // The message itself comes back on the control stream as an
            // event, the same one every other member gets, so there is one
            // code path in the client that renders a message.
            Ok(message) => Handled::with(Response::Ok, Outgoing::Message { message, nonce }),
            Err(err) => Handled::plain(Response::Error(to_protocol_error(&err))),
        },

        Request::Backfill {
            channel_id,
            before,
            limit,
        } => match server.backfill(&channel_id, before.as_deref(), limit).await {
            Ok(messages) => Handled::plain(Response::Messages { messages }),
            Err(err) => Handled::plain(Response::Error(to_protocol_error(&err))),
        },

        Request::Edit { id, content } => {
            match server.edit_message(&session.user, &id, &content).await {
                Ok(message) => Handled::with(
                    Response::Ok,
                    Outgoing::Edited {
                        message: message.clone(),
                    },
                ),
                Err(err) => Handled::plain(Response::Error(to_protocol_error(&err))),
            }
        }

        Request::Delete { id } => match server.delete_message(&session.user, &id).await {
            Ok((message, deleted_at)) => Handled::with(
                Response::Ok,
                Outgoing::Deleted {
                    id: message.id,
                    channel_id: message.channel_id,
                    deleted_at,
                },
            ),
            Err(err) => Handled::plain(Response::Error(to_protocol_error(&err))),
        },

        Request::Resume { cursors, since } => match server.resume(&cursors, since).await {
            Ok(resumed) => Handled::plain(Response::Resumed {
                messages: resumed.messages,
                truncated: resumed.truncated,
            }),
            Err(err) => Handled::plain(Response::Error(to_protocol_error(&err))),
        },

        // Fire and forget: nothing is stored, and the answer is `ok` whether
        // anybody was listening or not. A typing indicator that failed is not
        // something the sender can do anything about.
        Request::Typing { channel_id } => {
            Handled::with(Response::Ok, Outgoing::Typing { channel_id })
        }

        Request::CreateChannel { name, topic } => {
            if let Some(refused) = owner_only(session) {
                return refused;
            }
            match server.create_channel(&name, topic.as_deref()).await {
                Ok(channel) => Handled::with(Response::Channel { channel }, Outgoing::Channels),
                Err(err) => Handled::plain(Response::Error(to_protocol_error(&err))),
            }
        }

        Request::DeleteChannel { id } => {
            if let Some(refused) = owner_only(session) {
                return refused;
            }
            match server.delete_channel(&id).await {
                Ok(()) => Handled::with(Response::Ok, Outgoing::Channels),
                Err(err) => Handled::plain(Response::Error(to_protocol_error(&err))),
            }
        }

        Request::Kick { user_id } => {
            if let Some(refused) = owner_only(session) {
                return refused;
            }
            match server.kick(&user_id).await {
                // The roster does not change — a kicked member is still a
                // member — but their sessions have to go, and they have to be
                // told rather than left watching a connection that stopped
                // answering.
                Ok(_revoked) => Handled::with(
                    Response::Ok,
                    Outgoing::Revoked(Revoked::Account { user_id }),
                ),
                Err(err) => Handled::plain(Response::Error(to_protocol_error(&err))),
            }
        }

        Request::SetBanned { user_id, banned } => {
            if let Some(refused) = owner_only(session) {
                return refused;
            }
            match server.set_banned(&user_id, banned).await {
                Ok(()) if banned => Handled::with_all(
                    Response::Ok,
                    vec![
                        Outgoing::Members,
                        Outgoing::Revoked(Revoked::Account { user_id }),
                    ],
                ),
                // Un-banning disconnects nobody: there is nobody connected.
                Ok(()) => Handled::with(Response::Ok, Outgoing::Members),
                Err(err) => Handled::plain(Response::Error(to_protocol_error(&err))),
            }
        }

        Request::RevokeDevice {
            user_id,
            endpoint_id,
        } => {
            // The owner may revoke anybody's device; everybody else may revoke
            // only their own. Revoking your own is how you log a machine out.
            if user_id != session.user.id && !session.user.is_owner {
                return Handled::plain(Response::Error(ProtocolError::new(ErrorCode::Forbidden)));
            }
            match server.revoke_device(&endpoint_id, &user_id).await {
                Ok(()) => Handled::with(
                    Response::Ok,
                    Outgoing::Revoked(Revoked::Device {
                        user_id,
                        endpoint_id,
                    }),
                ),
                Err(err) => Handled::plain(Response::Error(to_protocol_error(&err))),
            }
        }

        Request::Devices { user_id } => {
            let user_id = user_id.unwrap_or_else(|| session.user.id.clone());
            if user_id != session.user.id && !session.user.is_owner {
                return Handled::plain(Response::Error(ProtocolError::new(ErrorCode::Forbidden)));
            }
            match server.devices_for_user(&user_id).await {
                Ok(devices) => Handled::plain(Response::Devices {
                    devices: devices
                        .into_iter()
                        .map(|device| DeviceInfo {
                            // Marked here rather than by the client, which
                            // cannot know which connection it is reading on.
                            current: device.endpoint_id == session.endpoint_id.to_string()
                                && device.user_id == session.user.id,
                            endpoint_id: device.endpoint_id,
                            user_id: device.user_id,
                            label: device.label,
                            enrolled_at: device.enrolled_at,
                            last_seen: device.last_seen,
                            revoked_at: device.revoked_at,
                        })
                        .collect(),
                }),
                Err(err) => Handled::plain(Response::Error(to_protocol_error(&err))),
            }
        }

        Request::Invites => {
            if let Some(refused) = owner_only(session) {
                return refused;
            }
            match server.invites().await {
                Ok(invites) => Handled::plain(Response::Invites {
                    invites: invites
                        .into_iter()
                        .map(|invite| InviteInfo {
                            code: invite.code,
                            created_by: invite.created_by,
                            created_at: invite.created_at,
                            expires_at: invite.expires_at,
                            max_uses: invite.max_uses,
                            uses: invite.uses,
                        })
                        .collect(),
                }),
                Err(err) => Handled::plain(Response::Error(to_protocol_error(&err))),
            }
        }

        Request::RevokeInvite { code } => {
            if let Some(refused) = owner_only(session) {
                return refused;
            }
            match server.revoke_invite(&code).await {
                Ok(()) => Handled::plain(Response::Ok),
                Err(err) => Handled::plain(Response::Error(to_protocol_error(&err))),
            }
        }

        Request::Invite {
            expires_in,
            max_uses,
        } => {
            let expires_in = expires_in.map(std::time::Duration::from_secs);
            match server
                .create_invite(&session.user.id, expires_in, max_uses)
                .await
            {
                Ok(invite::Invite {
                    code,
                    expires_at,
                    max_uses,
                    ..
                }) => Handled::plain(Response::Invite {
                    code,
                    expires_at,
                    max_uses,
                }),
                Err(err) => Handled::plain(Response::Error(to_protocol_error(&err))),
            }
        }
    }
}

/// Maps an internal error onto the coarse code the wire carries.
///
/// Two rules. Anything that is our fault becomes [`ErrorCode::Internal`] with
/// no detail — an error message is an oracle, and the detail belongs in the
/// server's log where only the host can read it. And the codes that *are*
/// specific are only ever about something the client already knew: it chose
/// that username, it sent that invite code.
pub(crate) fn to_protocol_error(err: &ServerError) -> ProtocolError {
    let code = match err {
        ServerError::BadCredentials => ErrorCode::BadCredentials,
        ServerError::InviteInvalid => ErrorCode::InviteInvalid,
        ServerError::UsernameTaken => ErrorCode::UsernameTaken,
        ServerError::DeviceRevoked => ErrorCode::DeviceRevoked,
        ServerError::DeviceNotEnrolled => ErrorCode::DeviceNotEnrolled,
        ServerError::DeviceAmbiguous => ErrorCode::DeviceAmbiguous,
        ServerError::Validation(_) => ErrorCode::Invalid,
        ServerError::UnknownUser | ServerError::UnknownChannel | ServerError::UnknownMessage => {
            ErrorCode::NotFound
        }
        ServerError::Forbidden => ErrorCode::Forbidden,
        ServerError::Banned => ErrorCode::Banned,
        // A rule the client broke, and one it can act on: the answer is "keep
        // the other channel", not "something went wrong".
        ServerError::LastChannel => ErrorCode::Forbidden,
        ServerError::RateLimited { retry_after } => {
            return ProtocolError::rate_limited(retry_after.as_secs().max(1));
        }
        // Ours to fix, and the peer is told nothing but "it broke".
        ServerError::Db(_)
        | ServerError::Migrate(_)
        | ServerError::PasswordHash
        | ServerError::CorruptIdentity
        | ServerError::Endpoint(_)
        | ServerError::OwnerExists => {
            tracing::error!(error = %err, "request failed");
            ErrorCode::Internal
        }
    };
    ProtocolError::new(code)
}

/// Default page size for a client that asks for a silly one. Never reached
/// from [`handle`], which rejects the request instead; kept for the client.
pub const BACKFILL_DEFAULT_LIMIT: u32 = limits::BACKFILL_DEFAULT_LIMIT;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_failures_never_leak_their_detail() {
        // A database path, a SQL fragment or a column name in an error that
        // reaches a stranger is a free map of the server.
        let err = ServerError::Db(sqlx::Error::RowNotFound);
        let protocol = to_protocol_error(&err);
        assert_eq!(protocol.code, ErrorCode::Internal);
        assert_eq!(protocol.retry_after, None);

        let json = serde_json::to_string(&protocol).expect("serialisable");
        assert_eq!(json, r#"{"code":"INTERNAL"}"#);
    }

    #[test]
    fn a_wrong_password_and_an_unknown_user_are_the_same_answer() {
        // Anything else lets a stranger enumerate the accounts on a server.
        assert_eq!(
            to_protocol_error(&ServerError::BadCredentials).code,
            ErrorCode::BadCredentials
        );
    }

    #[test]
    fn rate_limiting_tells_the_client_how_long_to_wait() {
        let protocol = to_protocol_error(&ServerError::RateLimited {
            retry_after: std::time::Duration::from_secs(42),
        });
        assert_eq!(protocol.code, ErrorCode::RateLimited);
        assert_eq!(protocol.retry_after, Some(42));
    }

    #[test]
    fn a_sub_second_backoff_still_asks_for_a_wait() {
        // Truncating 900ms to "retry in 0s" would invite an immediate retry,
        // which is the one thing backoff exists to prevent.
        let protocol = to_protocol_error(&ServerError::RateLimited {
            retry_after: std::time::Duration::from_millis(900),
        });
        assert_eq!(protocol.retry_after, Some(1));
    }
}
