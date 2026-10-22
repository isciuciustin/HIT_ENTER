// Test code throughout; clippy.toml's exemption covers only `#[test]` fns.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use hit_event::{DeviceKey, Person, Verified, sign_binding, sign_revocation};
use hit_store::{Store, Stored};
use nostr::prelude::{EventBuilder, Filter, Kind, SingleLetterTag, Tag, Timestamp};

async fn store() -> (Store, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    (Store::open(&dir.path().join("node.db")).await.unwrap(), dir)
}

fn note(p: &Person, text: &str, at: u64, tags: Vec<Tag>) -> Verified {
    Verified::check(
        p.sign(
            EventBuilder::new(Kind::TextNote, text)
                .custom_created_at(Timestamp::from(at))
                .tags(tags),
        )
        .unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn insert_is_idempotent_and_survives_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("node.db");
    let p = Person::generate();
    let n = note(&p, "hi", 100, vec![]);
    {
        let s = Store::open(&path).await.unwrap();
        assert_eq!(s.insert(&n).await.unwrap(), Stored::New);
        assert_eq!(s.insert(&n).await.unwrap(), Stored::Duplicate);
    }
    let s = Store::open(&path).await.unwrap();
    assert_eq!(s.count().await.unwrap(), 1);
    assert_eq!(s.by_ids(&[n.id]).await.unwrap(), vec![n.into_event()]);
}

#[tokio::test]
async fn a_replaceable_kind_keeps_only_the_newest() {
    let (s, _d) = store().await;
    let p = Person::generate();
    let profile = |at: u64, name: &str| {
        Verified::check(
            p.sign(EventBuilder::new(Kind::Metadata, name).custom_created_at(Timestamp::from(at)))
                .unwrap(),
        )
        .unwrap()
    };
    assert_eq!(s.insert(&profile(10, "old")).await.unwrap(), Stored::New);
    assert_eq!(s.insert(&profile(20, "new")).await.unwrap(), Stored::New);
    assert_eq!(
        s.insert(&profile(15, "between")).await.unwrap(),
        Stored::Stale
    );
    let all = s.query(&Filter::new().kind(Kind::Metadata)).await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].content, "new");
}

#[tokio::test]
async fn a_revocation_replaces_a_binding_and_a_same_second_binding_cannot_undo_it() {
    let (s, _d) = store().await;
    let p = Person::generate();
    let d = DeviceKey::generate();
    let now = Timestamp::now();
    let b = Verified::check(sign_binding(&p, &d, now, hit_event::binding::DEFAULT_TTL).unwrap())
        .unwrap();
    let r = Verified::check(sign_revocation(&p, d.public(), now).unwrap()).unwrap();
    assert_eq!(s.insert(&b).await.unwrap(), Stored::New);
    assert_eq!(s.insert(&r).await.unwrap(), Stored::New);
    assert_eq!(s.insert(&b).await.unwrap(), Stored::Stale);
    let held = s
        .query(&Filter::new().kind(hit_event::DEVICE_BINDING))
        .await
        .unwrap();
    assert_eq!(held, vec![r.into_event()]);
}

#[tokio::test]
async fn ephemeral_events_are_not_stored() {
    let (s, _d) = store().await;
    let p = Person::generate();
    let e = Verified::check(
        p.sign(EventBuilder::new(Kind::from(20_001u16), "x"))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(s.insert(&e).await.unwrap(), Stored::Ephemeral);
    assert_eq!(s.count().await.unwrap(), 0);
}

#[tokio::test]
async fn filters_select_by_every_field() {
    let (s, _d) = store().await;
    let (a, b) = (Person::generate(), Person::generate());
    let h = |g: &str| Tag::custom("h", [g]);
    for (p, at, tags) in [
        (&a, 100, vec![h("g1")]),
        (&a, 200, vec![h("g2")]),
        (&b, 300, vec![h("g1")]),
        (&b, 400, vec![]),
    ] {
        s.insert(&note(p, &at.to_string(), at, tags)).await.unwrap();
    }
    let contents = |evs: Vec<nostr::prelude::Event>| -> Vec<String> {
        evs.into_iter().map(|e| e.content).collect()
    };
    let group = Filter::new().custom_tag(SingleLetterTag::from_char('h').unwrap(), "g1");
    assert_eq!(contents(s.query(&group).await.unwrap()), ["300", "100"]);
    assert_eq!(
        contents(
            s.query(&Filter::new().author(a.public_key()))
                .await
                .unwrap()
        ),
        ["200", "100"]
    );
    assert_eq!(
        contents(
            s.query(
                &Filter::new()
                    .since(Timestamp::from(200))
                    .until(Timestamp::from(300))
            )
            .await
            .unwrap()
        ),
        ["300", "200"]
    );
    assert_eq!(
        contents(s.query(&Filter::new().limit(1)).await.unwrap()),
        ["400"]
    );
    assert!(
        s.query(&Filter::new().authors(Vec::<nostr::prelude::PublicKey>::new()))
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        s.query(&Filter::new().search("x"))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(s.id_times(&group).await.unwrap().len(), 2);
    assert_eq!(
        s.id_times(&Filter::new().limit(1)).await.unwrap().len(),
        4,
        "negentropy ignores limit"
    );
}

#[tokio::test]
async fn a_hostile_tag_value_is_data_not_sql() {
    let (s, _d) = store().await;
    let p = Person::generate();
    s.insert(&note(&p, "x", 1, vec![Tag::custom("h", ["g1"])]))
        .await
        .unwrap();
    let evil = Filter::new().custom_tag(SingleLetterTag::from_char('h').unwrap(), "g1') OR 1=1 --");
    assert!(s.query(&evil).await.unwrap().is_empty());
    assert_eq!(s.count().await.unwrap(), 1);
}

#[tokio::test]
async fn carried_groups_are_remembered_once_and_survive_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("node.db");
    let (g1, g2) = (
        hit_event::GroupKey::generate().public_key(),
        hit_event::GroupKey::generate().public_key(),
    );
    {
        let s = Store::open(&path).await.unwrap();
        s.carry_group(&g1, Timestamp::from(1)).await.unwrap();
        s.carry_group(&g2, Timestamp::from(2)).await.unwrap();
        s.carry_group(&g1, Timestamp::from(3)).await.unwrap();
    }
    let s = Store::open(&path).await.unwrap();
    assert_eq!(s.carried_groups().await.unwrap(), vec![g1, g2]);
}

/// A group's state is ordered by `seq`, not by clock, in the store as in
/// the fold: a snapshot signed on a slow clock still replaces an older one.
#[tokio::test]
async fn group_state_with_a_higher_seq_replaces_one_with_a_newer_clock() {
    let (s, _d) = store().await;
    let key = hit_event::GroupKey::generate();
    let snap = |seq| hit_event::group::Snapshot {
        seq,
        ..Default::default()
    };
    let one = snap(1).sign(&key, Timestamp::from(100)).unwrap();
    let two = snap(2).sign(&key, Timestamp::from(50)).unwrap();
    for ev in one.into_iter().chain(two) {
        s.insert(&Verified::check(ev).unwrap()).await.unwrap();
    }
    let held = s
        .query(&Filter::new().kind(hit_event::group::GROUP_MEMBERS))
        .await
        .unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(hit_event::group::seq_of(&held[0]), Some(2));
}

#[tokio::test]
async fn a_deletion_removes_only_its_authors_own_events_of_the_kinds_allowed() {
    let (s, _dir) = store().await;
    let (alice, mallory) = (Person::generate(), Person::generate());
    let g = Person::generate().public_key();
    let v = |p: &Person, b: EventBuilder| Verified::check(p.sign(b).unwrap()).unwrap();
    let msg = v(&alice, hit_event::group::message(&g, "general", "oops"));
    let join = v(&alice, hit_event::group::join(&g, "code"));
    for e in [&msg, &join] {
        s.insert(e).await.unwrap();
    }
    let kinds = [hit_event::group::CHAT_MESSAGE, hit_event::group::REACTION];

    // Somebody else's deletion of Alice's message: stored, applies to nothing.
    let theirs = v(
        &mallory,
        hit_event::group::deletion(&g, "general", &[msg.id]),
    );
    s.insert(&theirs).await.unwrap();
    assert!(s.apply_deletion(&theirs, &kinds).await.unwrap().is_empty());
    assert!(!s.is_deleted(&msg.id, &alice.public_key()).await.unwrap());

    // Alice's own, naming the message and her join: only the message goes.
    let mine = v(
        &alice,
        hit_event::group::deletion(&g, "general", &[msg.id, join.id]),
    );
    s.insert(&mine).await.unwrap();
    assert_eq!(s.apply_deletion(&mine, &kinds).await.unwrap(), vec![msg.id]);
    assert!(s.by_ids(&[msg.id]).await.unwrap().is_empty());
    assert_eq!(s.by_ids(&[join.id]).await.unwrap().len(), 1);
    // And the message arriving again is recognised as withdrawn.
    assert!(s.is_deleted(&msg.id, &alice.public_key()).await.unwrap());
}

#[tokio::test]
async fn files_are_indexed_with_their_message_and_the_cache_keeps_mine_then_the_newest() {
    let (s, _dir) = store().await;
    let p = Person::generate();
    let g = Person::generate().public_key();
    let file = |c: char, size: u64| hit_event::group::Attachment {
        hash: c.to_string().repeat(64),
        sha256: "e".repeat(64),
        size,
        mime: "text/plain".into(),
        name: format!("{c}.txt"),
    };
    let (a, b, c) = (file('a', 100), file('b', 100), file('c', 100));
    let msg = |files: &[hit_event::group::Attachment]| {
        Verified::check(
            p.sign(hit_event::group::message_with(&g, "general", "", files))
                .unwrap(),
        )
        .unwrap()
    };
    let m1 = msg(&[a.clone(), b.clone()]);
    let m2 = msg(std::slice::from_ref(&c));
    s.insert(&m1).await.unwrap();
    s.insert(&m2).await.unwrap();
    assert_eq!(s.carrying(&a.hash).await.unwrap().len(), 1);
    assert_eq!(s.attached().await.unwrap().len(), 3);

    // Used: c long ago, b just now; a is mine.
    s.touch_blob(&a.hash, true).await.unwrap();
    s.touch_blob(&c.hash, false).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    s.touch_blob(&b.hash, false).await.unwrap();
    // Room for two: mine, then the newest.
    let keep = s.blobs_to_keep(200).await.unwrap();
    assert_eq!(keep, vec![a.hash.clone(), b.hash.clone()]);
    // Room for none: mine all the same.
    assert_eq!(s.blobs_to_keep(0).await.unwrap(), vec![a.hash.clone()]);

    // The message goes: so do its files, from the index and the live set.
    s.remove(&[m1.id]).await.unwrap();
    assert!(s.carrying(&a.hash).await.unwrap().is_empty());
    assert_eq!(s.blobs_to_keep(1_000).await.unwrap(), vec![c.hash.clone()]);
}
