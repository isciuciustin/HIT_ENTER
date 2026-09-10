//! M1's acceptance test: *"registers a user, enrols a device, and rejects a bad
//! password"* — plus the neighbouring rules that are easy to get wrong and
//! expensive to get wrong late.
//!
//! Everything here is a direct function call against a real SQLite file in a
//! temp directory (PLAN §14). There is no network in this milestone, and no
//! mock either: the database is the thing being tested.

use std::time::Duration;

use he_proto::Password;
use he_server::error::ServerError;
use he_server::{Result, Server};
use iroh::{EndpointId, SecretKey};

/// A server on a real file, deleted when the test ends.
struct TestSpace {
    server: Server,
    _dir: tempfile::TempDir,
}

impl std::ops::Deref for TestSpace {
    type Target = Server;
    fn deref(&self) -> &Server {
        &self.server
    }
}

async fn space() -> Result<TestSpace> {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = Server::open(&dir.path().join("server.db"), "Test Space").await?;
    Ok(TestSpace { server, _dir: dir })
}

/// Stands in for a device: in production this comes off the iroh connection,
/// never out of a message body (PLAN §11).
fn device() -> EndpointId {
    SecretKey::generate().public()
}

fn password(text: &str) -> Password {
    Password::new(text)
}

/// Creates the owner and an invite in one step; most tests need both.
async fn space_with_owner() -> Result<(TestSpace, String, String)> {
    let space = space().await?;
    let owner = space
        .create_owner("justin", &password("hosting is a hobby"))
        .await?;
    let invite = space.create_invite(&owner.id, None, None).await?;
    Ok((space, owner.id, invite.code))
}

#[tokio::test]
async fn registers_a_user_enrolls_a_device_and_rejects_a_bad_password() -> Result<()> {
    let (space, _owner_id, invite) = space_with_owner().await?;
    let laptop = device();

    let (user, enrolment) = space
        .register(
            &laptop,
            &invite,
            "ada",
            &password("difference engine"),
            Some("Ada's laptop"),
        )
        .await?;

    assert_eq!(user.username, "ada");
    assert!(!user.is_owner, "an invited member is not the owner");
    assert_eq!(enrolment.endpoint_id, laptop.to_string());
    assert!(enrolment.is_active());

    // The enrolled device now gets in with no password at all — this is the
    // whole point of PLAN §3.
    let (same_user, _) = space.authenticate_device(&laptop, None).await?;
    assert_eq!(same_user.id, user.id);

    // And the password still works, from anywhere.
    let phone = device();
    let (same_user, _) = space
        .login(&phone, "ada", &password("difference engine"), Some("phone"))
        .await?;
    assert_eq!(same_user.id, user.id);

    // A bad password is refused, and says nothing about why.
    let err = space
        .login(&device(), "ada", &password("difference engin"), None)
        .await
        .expect_err("a wrong password must not log anyone in");
    assert!(matches!(err, ServerError::BadCredentials), "{err:?}");

    Ok(())
}

#[tokio::test]
async fn an_unknown_username_is_indistinguishable_from_a_wrong_password() -> Result<()> {
    let (space, _owner_id, _invite) = space_with_owner().await?;

    let unknown = space
        .login(&device(), "nobody-here", &password("whatever it is"), None)
        .await
        .expect_err("must not log in");
    let wrong = space
        .login(&device(), "justin", &password("not the password"), None)
        .await
        .expect_err("must not log in");

    // Same variant, same string: a stranger cannot use failures to learn which
    // accounts exist on this server.
    assert!(matches!(unknown, ServerError::BadCredentials));
    assert!(matches!(wrong, ServerError::BadCredentials));
    assert_eq!(unknown.to_string(), wrong.to_string());
    Ok(())
}

#[tokio::test]
async fn registration_is_invite_gated() -> Result<()> {
    let (space, _owner_id, _invite) = space_with_owner().await?;

    for attempt in ["", "not-a-code", "K7QP-2M4X-9WTZ"] {
        let err = space
            .register(
                &device(),
                attempt,
                "mallory",
                &password("let me in now"),
                None,
            )
            .await
            .expect_err("registration without a valid invite must fail");
        assert!(
            matches!(err, ServerError::InviteInvalid),
            "{attempt}: {err:?}"
        );
    }

    assert!(space.user_by_username("mallory").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn an_invite_runs_out_and_a_failed_registration_does_not_burn_a_use() -> Result<()> {
    let space = space().await?;
    let owner = space
        .create_owner("justin", &password("hosting is a hobby"))
        .await?;
    let invite = space.create_invite(&owner.id, None, Some(1)).await?;

    // A registration that fails *after* the invite is checked must roll the
    // redemption back, or a typo would cost someone their only invite.
    let err = space
        .register(
            &device(),
            &invite.code,
            "justin",
            &password("taken already"),
            None,
        )
        .await
        .expect_err("the username is taken");
    assert!(matches!(err, ServerError::UsernameTaken), "{err:?}");

    // Still usable, exactly once.
    space
        .register(
            &device(),
            &invite.code,
            "ada",
            &password("difference engine"),
            None,
        )
        .await?;
    let err = space
        .register(
            &device(),
            &invite.code,
            "grace",
            &password("nanoseconds!!"),
            None,
        )
        .await
        .expect_err("max_uses = 1");
    assert!(matches!(err, ServerError::InviteInvalid), "{err:?}");

    Ok(())
}

#[tokio::test]
async fn an_expired_invite_is_refused() -> Result<()> {
    let space = space().await?;
    let owner = space
        .create_owner("justin", &password("hosting is a hobby"))
        .await?;

    // `create_invite` takes a duration from now; ask for one that has already
    // run out rather than sleeping through a real one.
    let invite = space
        .create_invite(&owner.id, Some(Duration::ZERO), None)
        .await?;
    let err = space
        .register(
            &device(),
            &invite.code,
            "ada",
            &password("difference engine"),
            None,
        )
        .await
        .expect_err("an expired invite must not work");
    assert!(matches!(err, ServerError::InviteInvalid), "{err:?}");
    Ok(())
}

#[tokio::test]
async fn an_invite_code_is_case_and_dash_insensitive() -> Result<()> {
    let (space, _owner_id, invite) = space_with_owner().await?;

    // Codes get read aloud and retyped; the ones that differ only in shape are
    // the same code.
    let mangled = invite.to_lowercase().replace('-', " ");
    space
        .register(
            &device(),
            &mangled,
            "ada",
            &password("difference engine"),
            None,
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn a_revoked_device_is_refused_and_the_password_re_enrols_it() -> Result<()> {
    let (space, _owner_id, invite) = space_with_owner().await?;
    let laptop = device();
    let (user, _) = space
        .register(
            &laptop,
            &invite,
            "ada",
            &password("difference engine"),
            None,
        )
        .await?;

    space.revoke_device(&laptop.to_string(), &user.id).await?;

    let err = space
        .authenticate_device(&laptop, None)
        .await
        .expect_err("a revoked device must not get in");
    assert!(matches!(err, ServerError::DeviceRevoked), "{err:?}");

    // Revoking kicks a device out; it does not blacklist the key. Getting back
    // in still costs the password, which is the property that matters.
    space
        .login(&laptop, "ada", &password("difference engine"), None)
        .await?;
    let (same_user, _) = space.authenticate_device(&laptop, None).await?;
    assert_eq!(same_user.id, user.id);

    Ok(())
}

#[tokio::test]
async fn an_unenrolled_device_gets_nothing_for_knowing_the_endpoint_id() -> Result<()> {
    let (space, _owner_id, _invite) = space_with_owner().await?;

    // Accepting a connection is not authorization (PLAN §11).
    let err = space
        .authenticate_device(&device(), None)
        .await
        .expect_err("a stranger's key must not authenticate");
    assert!(matches!(err, ServerError::DeviceNotEnrolled), "{err:?}");
    Ok(())
}

#[tokio::test]
async fn one_device_two_accounts_needs_the_username() -> Result<()> {
    let space = space().await?;
    let owner = space
        .create_owner("justin", &password("hosting is a hobby"))
        .await?;
    let invite = space.create_invite(&owner.id, None, None).await?;

    let shared = device();
    space
        .register(
            &shared,
            &invite.code,
            "ada",
            &password("difference engine"),
            None,
        )
        .await?;
    space
        .register(
            &shared,
            &invite.code,
            "grace",
            &password("nanoseconds!!"),
            None,
        )
        .await?;

    // The key alone cannot say which account this connection means, and
    // guessing would log someone in as the wrong person.
    let err = space
        .authenticate_device(&shared, None)
        .await
        .expect_err("ambiguous");
    assert!(matches!(err, ServerError::DeviceAmbiguous), "{err:?}");

    let (user, _) = space.authenticate_device(&shared, Some("grace")).await?;
    assert_eq!(user.username, "grace");
    Ok(())
}

#[tokio::test]
async fn usernames_are_unique_case_insensitively() -> Result<()> {
    let (space, _owner_id, invite) = space_with_owner().await?;
    space
        .register(
            &device(),
            &invite,
            "Ada",
            &password("difference engine"),
            None,
        )
        .await?;

    // Otherwise `Ada` and `ada` are two accounts that look identical in the
    // member list, which is an impersonation kit.
    let err = space
        .register(
            &device(),
            &invite,
            "aDa",
            &password("difference engine"),
            None,
        )
        .await
        .expect_err("must be taken");
    assert!(matches!(err, ServerError::UsernameTaken), "{err:?}");

    let found = space
        .user_by_username("ADA")
        .await?
        .expect("case-insensitive");
    assert_eq!(
        found.username, "Ada",
        "the display form is preserved as typed"
    );
    Ok(())
}

#[tokio::test]
async fn repeated_wrong_passwords_start_costing_time() -> Result<()> {
    let (space, _owner_id, _invite) = space_with_owner().await?;
    let attacker = device();

    let mut rate_limited = None;
    for attempt in 0..6 {
        match space
            .login(&attacker, "justin", &password("guess number one"), None)
            .await
        {
            Err(ServerError::BadCredentials) => {}
            Err(err @ ServerError::RateLimited { .. }) => {
                rate_limited = Some((attempt, err));
                break;
            }
            other => panic!("attempt {attempt}: unexpected {other:?}"),
        }
    }

    let (attempt, err) = rate_limited.expect("guessing must eventually be throttled");
    assert!(
        attempt >= 3,
        "typos must stay free: throttled after {attempt}"
    );
    assert!(err.to_string().contains("retry in"), "{err}");

    // The owner's real password is not what is being punished — but this
    // device is, until the backoff expires.
    let err = space
        .login(&attacker, "justin", &password("hosting is a hobby"), None)
        .await
        .expect_err("still backing off");
    assert!(matches!(err, ServerError::RateLimited { .. }), "{err:?}");

    // A different device with the *correct* password is unaffected... except
    // that the username itself is also being throttled, which is deliberate:
    // an attacker must not sidestep the limit by making a new key.
    let err = space
        .login(&device(), "justin", &password("hosting is a hobby"), None)
        .await
        .expect_err("the username is throttled too");
    assert!(matches!(err, ServerError::RateLimited { .. }), "{err:?}");

    Ok(())
}

#[tokio::test]
async fn a_server_has_exactly_one_owner_and_it_is_created_locally() -> Result<()> {
    let space = space().await?;
    let owner = space
        .create_owner("justin", &password("hosting is a hobby"))
        .await?;
    assert!(owner.is_owner);

    let err = space
        .create_owner("someone-else", &password("also wants it"))
        .await
        .expect_err("a space has one owner");
    assert!(matches!(err, ServerError::OwnerExists), "{err:?}");
    Ok(())
}

#[tokio::test]
async fn short_passwords_and_bad_usernames_are_refused_at_the_door() -> Result<()> {
    let (space, _owner_id, invite) = space_with_owner().await?;

    let err = space
        .register(&device(), &invite, "ada", &password("short"), None)
        .await
        .expect_err("too short");
    assert!(matches!(err, ServerError::Validation(_)), "{err:?}");

    let err = space
        .register(
            &device(),
            &invite,
            "ada lovelace",
            &password("difference engine"),
            None,
        )
        .await
        .expect_err("space in username");
    assert!(matches!(err, ServerError::Validation(_)), "{err:?}");

    Ok(())
}

#[tokio::test]
async fn the_database_never_holds_a_password() -> Result<()> {
    let (space, _owner_id, invite) = space_with_owner().await?;
    let secret = "correct horse battery staple";
    space
        .register(&device(), &invite, "ada", &password(secret), None)
        .await?;

    // PLAN §10: hashing is one-way, and nothing in this system can undo it.
    let hashes: Vec<String> = sqlx::query_scalar("SELECT password_hash FROM users")
        .fetch_all(space.pool())
        .await?;
    assert_eq!(hashes.len(), 2);
    for hash in hashes {
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        assert!(!hash.contains(secret));
        assert!(!hash.contains("hosting is a hobby"));
    }
    Ok(())
}

#[tokio::test]
async fn the_identity_and_the_accounts_survive_a_restart() -> Result<()> {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("server.db");

    let (endpoint_id, user_id, laptop) = {
        let server = Server::open(&path, "Test Space").await?;
        let owner = server
            .create_owner("justin", &password("hosting is a hobby"))
            .await?;
        let invite = server.create_invite(&owner.id, None, None).await?;
        let laptop = device();
        let (user, _) = server
            .register(
                &laptop,
                &invite.code,
                "ada",
                &password("difference engine"),
                None,
            )
            .await?;
        (server.endpoint_id(), user.id, laptop)
    };

    let server = Server::open(&path, "A Different Name").await?;
    assert_eq!(
        server.endpoint_id(),
        endpoint_id,
        "losing the secret key would kill every invite ticket ever issued"
    );
    assert_eq!(
        server.name(),
        "Test Space",
        "an existing space keeps its name"
    );

    // The device is still enrolled, so reconnecting costs nothing.
    let (user, _) = server.authenticate_device(&laptop, None).await?;
    assert_eq!(user.id, user_id);
    Ok(())
}

#[tokio::test]
async fn an_owner_can_see_and_revoke_the_devices_on_an_account() -> Result<()> {
    let (space, _owner_id, invite) = space_with_owner().await?;
    let laptop = device();
    let phone = device();

    let (user, _) = space
        .register(
            &laptop,
            &invite,
            "ada",
            &password("difference engine"),
            Some("laptop"),
        )
        .await?;
    space
        .login(&phone, "ada", &password("difference engine"), Some("phone"))
        .await?;

    let enrolled = space.devices_for_user(&user.id).await?;
    assert_eq!(enrolled.len(), 2);
    let labels: Vec<Option<String>> = enrolled.iter().map(|d| d.label.clone()).collect();
    assert!(labels.contains(&Some("laptop".to_string())));
    assert!(labels.contains(&Some("phone".to_string())));

    space.revoke_device(&phone.to_string(), &user.id).await?;

    // Revoked devices stay in the list: an owner needs to see what was there.
    let enrolled = space.devices_for_user(&user.id).await?;
    assert_eq!(enrolled.len(), 2);
    assert_eq!(enrolled.iter().filter(|d| d.is_active()).count(), 1);

    // The other device is untouched.
    space.authenticate_device(&laptop, None).await?;
    Ok(())
}
