//! Turning a [`Request`] into a [`Response`].
//!
//! Everything here is a thin translation onto [`Server`] calls that M1 already
//! shipped and tested without a network. That is the point of the split: this
//! module decides *nothing*. If it grew a rule of its own, that rule would be
//! reachable only over a socket and testable only over a socket.

use he_proto::rpc::{ErrorCode, ProtocolError, Request, Response};
use he_proto::{Message, limits};

use crate::accept::Session;
use crate::error::ServerError;
use crate::{Server, invite};

/// What a handled [`Request::Send`] produced, for the caller to broadcast.
pub(crate) struct Handled {
    pub response: Response,
    /// Set only by a successful `send`. The nonce goes back to the sender
    /// alone; the message goes to everyone (PLAN §9).
    pub broadcast: Option<(Message, String)>,
}

impl Handled {
    fn plain(response: Response) -> Self {
        Self {
            response,
            broadcast: None,
        }
    }
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
            Ok(message) => Handled {
                response: Response::Ok,
                broadcast: Some((message, nonce)),
            },
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
        ServerError::UnknownUser | ServerError::UnknownChannel => ErrorCode::NotFound,
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
