//! What the window knows, in one place — `web/src/lib/chat.svelte.ts`,
//! ported: the groups, the open group and channel, its messages and
//! members, names, who is here, unread counts. hit-app-core is the record;
//! this is a cache of it, refreshed by its events and thrown away on a
//! resync.
//!
//! Every change ends in [`Chat::render`], which turns the cache into the
//! window's rows and hands them to Slint's event loop. The message list is
//! patched row by row rather than replaced, so a message arriving while
//! someone reads further up does not move what they are reading.

mod dms;
pub mod voice;
pub mod zaps;

use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap, HashSet},
    future::Future,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Weak},
};

use hit_app_core::{
    events::UiEvent,
    state::{App, BanView, DmView, FileView, GroupView, MemberView, MessageView, ReactionView},
};
use slint::{Model, ModelRc, SharedString, VecModel};
use tokio::sync::Mutex;

use crate::{
    pictures::{self, Pictures, Pixels},
    ui::{
        BanUi, ChannelUi, ChipUi, FileUi, GroupUi, MainWindow, MemberUi, MessageUi, PendingUi,
        CommunityUi, PersonUi, RoleUi, SettingsUi, ViewUi,
    },
};

/// Same author within five minutes: one block, one name.
const CONTINUES_SECS: u64 = 300;

pub struct Chat {
    app: Arc<App>,
    ui: slint::Weak<MainWindow>,
    s: Mutex<State>,
    /// Itself, for the tasks it starts (pictures load in the background).
    me: Weak<Chat>,
}

#[derive(Default)]
struct State {
    me: String,
    max_chars: usize,
    groups: Vec<GroupView>,
    group: Option<String>,
    channel: Option<String>,
    messages: Vec<MessageView>,
    members: Vec<MemberView>,
    /// npub → display name, for everyone this window has asked about.
    names: HashMap<String, String>,
    /// npubs whose presence reached us lately, or whose device did.
    present: BTreeSet<String>,
    /// `group/channel` → messages from others after its read mark.
    unread: HashMap<String, usize>,
    /// Why the last send failed, shown under the composer.
    send_error: Option<String>,
    /// Why the last reaction, withdrawal or removal failed.
    message_error: Option<String>,
    /// The open group's bans, read when its settings open.
    bans: Vec<BanView>,
    /// The member whose dialog is open.
    person: Option<String>,
    /// Files attached to the group composer, not sent yet.
    pending: Vec<FileView>,
    /// Decoded pictures, and what is loading or failed.
    pics: Pictures,
    // ---- direct messages (chat/dms.rs) ----
    /// The DM pane is shown instead of a group.
    dms_open: bool,
    dms: Vec<DmView>,
    /// The open conversation's person.
    dm_with: Option<String>,
    /// npub → DMs from them after its read mark.
    dm_unread: HashMap<String, usize>,
    dm_error: Option<String>,
    // ---- voice (chat/voice.rs) ----
    voice: Option<hit_app_core::state::VoiceView>,
    voice_error: Option<String>,
    /// Joining, ringing or answering: opening the audio takes a moment.
    call_busy: bool,
    /// The talk key or button is held.
    talking: bool,
    // ---- zaps (chat/zaps.rs) ----
    /// The message the zap dialog is for: its group and id.
    zap_target: Option<(String, String)>,
    /// The window has the desktop's focus: only then is what it shows read.
    /// Unknown until told, and taken as yes.
    unfocused: bool,
    /// A task re-renders every second while the open group makes this
    /// person wait to post (M13's newcomer countdown).
    ticking: bool,
}

/// Seconds `group` still makes this person wait before posting, if any.
fn newcomer_left(group: Option<&GroupView>) -> Option<u64> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    group
        .and_then(|g| g.posts_from)
        .filter(|&t| t > now)
        .map(|t| t - now)
}

impl Chat {
    pub fn new(app: Arc<App>, ui: slint::Weak<MainWindow>) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            app,
            ui,
            s: Mutex::default(),
            me: me.clone(),
        })
    }

    /// Re-reads everything from hit-app-core.
    pub async fn refresh(&self) {
        let mut s = self.s.lock().await;
        if let Ok(st) = self.app.status().await {
            s.me = st.npub.unwrap_or_default();
            s.max_chars = st.max_message_chars;
            if let Some(n) = st.name {
                let me = s.me.clone();
                s.names.insert(me, n);
            }
        }
        self.refresh_groups(&mut s).await;
        self.load_group(&mut s).await;
        s.present = self.app.present().await.unwrap_or_default();
        s.unread = self
            .app
            .unread()
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|u| (format!("{}/{}", u.group, u.channel), u.count))
            .collect();
        self.load_dms(&mut s).await;
        s.voice = self.app.voice_view().await.ok();
        self.mark_read(&mut s).await;
        self.render(&mut s);
    }

    pub async fn select_group(&self, id: String) {
        let mut s = self.s.lock().await;
        if s.group.as_deref() == Some(id.as_str()) {
            if s.dms_open {
                s.dms_open = false;
                self.mark_read(&mut s).await;
                self.render(&mut s);
            }
            return;
        }
        s.dms_open = false;
        s.channel = first_text(s.groups.iter().find(|g| g.id == id));
        s.group = Some(id);
        s.message_error = None;
        self.load_group(&mut s).await;
        self.mark_read(&mut s).await;
        self.render(&mut s);
    }

    pub async fn select_channel(&self, id: String) {
        let mut s = self.s.lock().await;
        if s.channel.as_deref() == Some(id.as_str()) {
            return;
        }
        s.channel = Some(id);
        s.message_error = None;
        self.load_messages(&mut s).await;
        self.mark_read(&mut s).await;
        self.render(&mut s);
    }

    /// Sends `text` to the open channel; whether it went.
    pub async fn send(&self, text: String) -> bool {
        let mut s = self.s.lock().await;
        let (Some(g), Some(c)) = (s.group.clone(), s.channel.clone()) else {
            return false;
        };
        s.send_error = None;
        let files = s.pending.clone();
        let sent = match self.app.send(&g, &c, &text, files).await {
            Ok(m) => {
                s.pending.clear();
                self.arrived(&mut s, m).await;
                true
            }
            Err(e) => {
                s.send_error = Some(e.to_string());
                false
            }
        };
        self.render(&mut s);
        sent
    }

    pub async fn create_group(&self, name: String, public: bool) -> Result<(), String> {
        let id = self
            .app
            .create_group(&name, public)
            .await
            .map_err(|e| e.to_string())?;
        self.opened(id).await;
        Ok(())
    }

    pub async fn join(&self, link: String) -> Result<(), String> {
        let id = self.app.join(&link).await.map_err(|e| e.to_string())?;
        self.opened(id).await;
        Ok(())
    }

    /// Public groups announced on the public relays (M13).
    pub async fn browse(&self, query: &str, fresh: bool) -> Result<Vec<CommunityUi>, String> {
        let found = self
            .app
            .browse(query, fresh)
            .await
            .map_err(|e| e.to_string())?;
        Ok(found
            .into_iter()
            .map(|c| CommunityUi {
                name: c.name.into(),
                about: c.about.into(),
                known: known_line(&c.known, c.known_count).into(),
                members: match c.members {
                    1 => "1 member".into(),
                    n => format!("{n} members").into(),
                },
                link: c.link.into(),
                joined: c.joined,
            })
            .collect())
    }

    async fn opened(&self, id: String) {
        let mut s = self.s.lock().await;
        self.refresh_groups(&mut s).await;
        s.channel = first_text(s.groups.iter().find(|g| g.id == id));
        s.group = Some(id);
        self.load_group(&mut s).await;
        self.render(&mut s);
    }

    /// An invite link to the open group.
    /// Joins the public group being read (M13); a failure shows under the
    /// composer.
    pub async fn join_reading(&self) {
        let g = self.s.lock().await.group.clone();
        let Some(g) = g else { return };
        let joined = self.app.join_reading(&g).await;
        let mut s = self.s.lock().await;
        s.send_error = joined.err().map(|e| e.to_string());
        self.refresh_groups(&mut s).await;
        self.render(&mut s);
    }

    /// A link to the open group: an invite valid for `hours`, or — a
    /// public group (M13) — its link with no invite in it.
    pub async fn invite(&self, hours: Option<u32>) -> Result<String, String> {
        let (g, public) = {
            let s = self.s.lock().await;
            let g = s.group.clone().ok_or("no group is open")?;
            let public = s.groups.iter().any(|x| x.id == g && x.public);
            (g, public)
        };
        if public {
            self.app.public_link(&g).await
        } else {
            self.app.invite(&g, hours).await
        }
        .map_err(|e| e.to_string())
    }

    /// One of the node's events.
    pub async fn on_event(&self, event: UiEvent) {
        let mut s = self.s.lock().await;
        match event {
            UiEvent::Message(m) => self.arrived(&mut s, m).await,
            UiEvent::Group(g) => {
                self.refresh_groups(&mut s).await;
                if s.group.as_deref() == Some(g.as_str()) {
                    // A channel I can no longer see is closed for me.
                    let still = s.groups.iter().find(|x| x.id == g).is_some_and(|x| {
                        x.channels
                            .iter()
                            .any(|c| Some(&c.id) == s.channel.as_ref() && c.kind == "text")
                    });
                    if !still {
                        s.channel = first_text(s.groups.iter().find(|x| x.id == g));
                        self.load_messages(&mut s).await;
                    }
                    self.load_members(&mut s).await;
                }
            }
            UiEvent::Profile(p) => {
                self.learn_names(&mut s, vec![p], true).await;
                // A profile names whose wallet signs someone's receipts: a
                // change can make zaps count, or stop counting.
                self.zap_totals(&mut s, None).await;
            }
            UiEvent::Present(p) => s.present = p.into_iter().collect(),
            UiEvent::Withdrawn(ids) => Self::gone(&mut s, &ids),
            UiEvent::Resync => {
                drop(s);
                return self.refresh().await;
            }
            UiEvent::Reaction(r) => Self::reacted(&mut s, r),
            UiEvent::Dm(d) => self.dm_arrived(&mut s, d).await,
            UiEvent::Voice(v) => self.voice_changed(&mut s, v).await,
            UiEvent::Ringing => {
                Self::ask_attention(&self.ui);
                return;
            }
            UiEvent::Zap(id) => self.zap_totals(&mut s, Some(vec![id])).await,
        }
        self.render(&mut s);
    }

    /// Toggles my `emoji` on message `id`: withdrawn if I had it, added if
    /// not.
    pub async fn react(&self, id: String, emoji: String) {
        let mut s = self.s.lock().await;
        let Some(m) = s.messages.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        let mine = m.reactions.iter().find(|r| r.mine && r.emoji == emoji);
        let outcome = match mine {
            Some(r) => self
                .app
                .withdraw(&m.group, &m.channel, &r.id)
                .await
                .map(|()| Self::gone(&mut s, std::slice::from_ref(&r.id))),
            None => self
                .app
                .react(&m.group, &m.channel, &m.id, &emoji)
                .await
                .map(|r| Self::reacted(&mut s, r)),
        };
        s.message_error = outcome.err().map(|e| e.to_string());
        self.render(&mut s);
    }

    /// Withdraws my own message `id` (NIP-09), or — `remove` — a
    /// moderator's removal of someone else's (`9005`).
    pub async fn take_back(&self, id: String, remove: bool) {
        let mut s = self.s.lock().await;
        let Some(m) = s.messages.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        let outcome = if remove {
            self.app.remove_message(&m.group, &m.channel, &m.id).await
        } else {
            self.app.withdraw(&m.group, &m.channel, &m.id).await
        };
        match outcome {
            Ok(()) => {
                Self::gone(&mut s, &[id]);
                s.message_error = None;
            }
            Err(e) => s.message_error = Some(e.to_string()),
        }
        self.render(&mut s);
    }

    fn reacted(s: &mut State, r: ReactionView) {
        if let Some(m) = s.messages.iter_mut().find(|m| m.id == r.message)
            && !m.reactions.iter().any(|x| x.id == r.id)
        {
            m.reactions.push(r);
        }
    }

    fn gone(s: &mut State, ids: &[String]) {
        s.messages.retain(|m| !ids.contains(&m.id));
        for m in &mut s.messages {
            m.reactions.retain(|r| !ids.contains(&r.id));
        }
    }

    /// The open group's settings are opening: who is banned.
    pub async fn open_settings(&self) {
        let mut s = self.s.lock().await;
        s.person = None;
        self.load_bans(&mut s).await;
        self.render(&mut s);
    }

    /// `npub`'s dialog is opening: the roles this person could hand them,
    /// and which they hold.
    pub async fn open_member(&self, npub: String) -> Vec<RoleUi> {
        let mut s = self.s.lock().await;
        let held: Vec<String> = s
            .members
            .iter()
            .find(|m| m.npub == npub)
            .map(|m| m.role_ids.clone())
            .unwrap_or_default();
        s.person = Some(npub);
        self.render(&mut s);
        let Some(g) = s.group.clone() else {
            return Vec::new();
        };
        drop(s);
        self.app
            .roles(&g)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| RoleUi {
                chosen: held.contains(&r.id),
                id: r.id.into(),
                name: r.name.into(),
                assignable: r.assignable,
            })
            .collect()
    }

    /// One moderation step on the open group, then the group, its members
    /// and its bans read again — every peer judges the action again; this
    /// node's judgement is the error, if any.
    pub async fn moderate<F, Fut>(&self, step: F) -> Result<(), String>
    where
        F: FnOnce(Arc<App>, String, Option<String>) -> Fut,
        Fut: Future<Output = Result<(), hit_app_core::state::AppError>>,
    {
        let (g, who) = {
            let s = self.s.lock().await;
            (s.group.clone(), s.person.clone())
        };
        let g = g.ok_or("no group is open")?;
        let outcome = step(self.app.clone(), g, who)
            .await
            .map_err(|e| e.to_string());
        let mut s = self.s.lock().await;
        self.refresh_groups(&mut s).await;
        self.load_members(&mut s).await;
        self.load_bans(&mut s).await;
        // Its channel deleted, or made private: open another.
        let open = s.groups.iter().find(|x| Some(&x.id) == s.group.as_ref());
        if !open.is_some_and(|g| g.channels.iter().any(|c| Some(&c.id) == s.channel.as_ref())) {
            s.channel = first_text(open);
            self.load_messages(&mut s).await;
        }
        self.render(&mut s);
        outcome
    }

    async fn load_bans(&self, s: &mut State) {
        let may = s
            .groups
            .iter()
            .find(|g| Some(&g.id) == s.group.as_ref())
            .is_some_and(|g| g.can_ban);
        s.bans = match s.group.clone().filter(|_| may) {
            Some(g) => self.app.bans(&g).await.unwrap_or_default(),
            None => Vec::new(),
        };
    }

    /// The window gained or lost the desktop's focus. Coming back means what
    /// is on screen has been seen.
    pub async fn window_focused(&self, on: bool) {
        let mut s = self.s.lock().await;
        s.unfocused = !on;
        if on {
            self.mark_read(&mut s).await;
            self.mark_dm_read(&mut s).await;
            self.render(&mut s);
        }
    }

    /// Adds files from disk to the group composer.
    pub async fn attach(&self, paths: Vec<PathBuf>) {
        let mut s = self.s.lock().await;
        s.send_error = None;
        for p in paths {
            match self.app.attach_file(&p).await {
                Ok(f) if !s.pending.iter().any(|x| x.hash == f.hash) => s.pending.push(f),
                Ok(_) => {}
                Err(e) => s.send_error = Some(e.to_string()),
            }
        }
        self.render(&mut s);
    }

    pub async fn unattach(&self, hash: String) {
        let mut s = self.s.lock().await;
        s.pending.retain(|f| f.hash != hash);
        self.render(&mut s);
    }

    /// *show* on a picture above the size shown without asking.
    pub async fn show_file(&self, key: String) {
        let mut s = self.s.lock().await;
        s.pics.ask(&key);
        self.render(&mut s);
    }

    /// Writes a file where the person chose.
    pub async fn save_file(&self, key: String, dm: bool, to: PathBuf) {
        let saved = if dm {
            self.app.export_dm_file(&key, &to).await
        } else {
            self.app.export_file(&key, &to).await
        };
        let mut s = self.s.lock().await;
        let note = match saved {
            Ok(()) => format!("saved to {}", to.display()),
            Err(e) => format!("not saved: {e}"),
        };
        s.pics.set_note(&key, note);
        self.render(&mut s);
    }

    /// The picture files on screen: the open channel's, or the open
    /// conversation's — `(key, dm, size)`.
    fn on_screen(s: &State) -> Vec<(String, bool, u64)> {
        if s.dms_open {
            let w = s.dm_with.as_deref();
            s.dms
                .iter()
                .filter(|d| Some(d.with.as_str()) == w)
                .filter_map(|d| {
                    let f = d.file.as_ref()?;
                    is_picture(&f.mime).then(|| (d.id.clone(), true, f.size))
                })
                .collect()
        } else {
            s.messages
                .iter()
                .flat_map(|m| &m.files)
                .filter(|f| is_picture(&f.mime))
                .map(|f| (f.hash.clone(), false, f.size))
                .collect()
        }
    }

    /// Starts loading every picture on screen that should show and is not
    /// held: fetched (from whoever has it), decoded at display size off the
    /// async workers, then the window drawn again.
    /// Starts the once-a-second re-render while a newcomer waits, so the
    /// composer counts down; it stops itself when the wait is over or the
    /// group is left for another.
    fn tick_while_waiting(&self, s: &mut State) {
        let open = s.groups.iter().find(|g| Some(&g.id) == s.group.as_ref());
        if s.ticking || newcomer_left(open).is_none() {
            return;
        }
        let Some(me) = self.me.upgrade() else { return };
        s.ticking = true;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let mut s = me.s.lock().await;
                let open = s.groups.iter().find(|g| Some(&g.id) == s.group.as_ref());
                let waiting = newcomer_left(open).is_some();
                if !waiting {
                    s.ticking = false;
                }
                // The last one shows the composer open again.
                me.render(&mut s);
                if !waiting {
                    break;
                }
            }
        });
    }

    fn fetch_pictures(&self, s: &mut State) {
        for (key, dm, size) in Self::on_screen(s) {
            if !s.pics.wants(&key, size) {
                continue;
            }
            s.pics.set_note(&key, "loading…".into());
            let Some(me) = self.me.upgrade() else { return };
            tokio::spawn(async move {
                let bytes = if dm {
                    me.app.dm_file_bytes(&key).await
                } else {
                    me.app.file_bytes(&key).await
                };
                let px = match bytes {
                    Ok(b) => {
                        let _turn = pictures::DECODING.acquire().await;
                        tokio::task::spawn_blocking(move || pictures::decode(&b))
                            .await
                            .unwrap_or_else(|e| Err(e.to_string()))
                    }
                    Err(e) => Err(e.to_string()),
                };
                let mut s = me.s.lock().await;
                match px {
                    Ok(px) => {
                        let visible: HashSet<String> =
                            Self::on_screen(&s).into_iter().map(|(k, ..)| k).collect();
                        s.pics.insert(&key, px, &visible);
                    }
                    Err(why) => s.pics.set_note(&key, why),
                }
                me.render(&mut s);
            });
        }
    }

    /// A file as the window shows it, before it reaches Slint's thread.
    fn file_line(s: &State, key: &str, dm: bool, name: &str, size: u64, mime: &str) -> FileLine {
        let pixels = s.pics.get(key).cloned();
        FileLine {
            key: key.into(),
            dm,
            name: name.into(),
            size: human_bytes(size).into(),
            image: is_picture(mime),
            note: s.pics.note(key).unwrap_or_default().into(),
            pixels,
        }
    }

    async fn arrived(&self, s: &mut State, m: MessageView) {
        let open = Some(&m.group) == s.group.as_ref() && Some(&m.channel) == s.channel.as_ref();
        if !s.names.contains_key(&m.author) {
            self.learn_names(s, vec![m.author.clone()], false).await;
        }
        if open {
            if s.messages.iter().any(|x| x.id == m.id) {
                return;
            }
            if (s.dms_open || s.unfocused) && !m.mine {
                // Its channel is open, but the DM pane covers it or the
                // window is in the background: unread.
                *s.unread
                    .entry(format!("{}/{}", m.group, m.channel))
                    .or_default() += 1;
            }
            s.messages.push(m);
            // Seconds are the clock's resolution: keep arrival order within one.
            let n = s.messages.len();
            if n > 1 && s.messages[n - 2].at > s.messages[n - 1].at {
                s.messages.sort_by_key(|x| x.at);
            }
            self.mark_read(s).await;
        } else if !m.mine {
            *s.unread
                .entry(format!("{}/{}", m.group, m.channel))
                .or_default() += 1;
        }
    }

    async fn refresh_groups(&self, s: &mut State) {
        s.groups = self.app.groups().await.unwrap_or_default();
        if !s.groups.iter().any(|g| Some(&g.id) == s.group.as_ref()) {
            let first = s.groups.iter().find(|g| g.ready).or(s.groups.first());
            s.group = first.map(|g| g.id.clone());
            s.channel = first_text(first);
        }
    }

    async fn load_group(&self, s: &mut State) {
        self.load_messages(s).await;
        self.load_members(s).await;
    }

    async fn load_messages(&self, s: &mut State) {
        let (Some(g), Some(c)) = (s.group.clone(), s.channel.clone()) else {
            s.messages.clear();
            return;
        };
        s.messages = self.app.messages(&g, &c).await.unwrap_or_default();
        let authors: BTreeSet<String> = s
            .messages
            .iter()
            .flat_map(|m| std::iter::once(&m.author).chain(m.reactions.iter().map(|r| &r.author)))
            .cloned()
            .collect();
        self.learn_names(s, authors.into_iter().collect(), false)
            .await;
    }

    async fn load_members(&self, s: &mut State) {
        let ready = s
            .groups
            .iter()
            .find(|g| Some(&g.id) == s.group.as_ref())
            .is_some_and(|g| g.ready);
        let Some(g) = s.group.clone().filter(|_| ready) else {
            s.members.clear();
            return;
        };
        s.members = self.app.members(&g).await.unwrap_or_default();
        for m in &s.members {
            if let Some(n) = &m.name {
                s.names.insert(m.npub.clone(), n.clone());
            }
        }
    }

    async fn learn_names(&self, s: &mut State, people: Vec<String>, force: bool) {
        let ask: Vec<String> = people
            .into_iter()
            .filter(|p| force || !s.names.contains_key(p))
            .collect();
        if ask.is_empty() {
            return;
        }
        if let Ok(got) = self.app.names(&ask).await {
            s.names.extend(got);
        }
    }

    /// The open channel is read up to its newest message — on disk too.
    async fn mark_read(&self, s: &mut State) {
        let (Some(g), Some(c)) = (s.group.clone(), s.channel.clone()) else {
            return;
        };
        if s.dms_open || s.unfocused {
            return;
        }
        s.unread.remove(&format!("{g}/{c}"));
        if let Some(last) = s.messages.last() {
            let _ = self.app.mark_read(&g, &c, last.at).await;
        }
    }

    fn name_of(s: &State, npub: &str) -> String {
        s.names
            .get(npub)
            .cloned()
            .unwrap_or_else(|| short_key(npub))
    }

    /// The cache, as the window's rows.
    fn render(&self, s: &mut State) {
        self.fetch_pictures(s);
        self.tick_while_waiting(s);
        let s = &*s;
        let group = s.groups.iter().find(|g| Some(&g.id) == s.group.as_ref());
        let channel = group.and_then(|g| {
            g.channels
                .iter()
                .find(|c| Some(&c.id) == s.channel.as_ref())
        });

        let groups: Vec<GroupUi> = s
            .groups
            .iter()
            .map(|g| GroupUi {
                id: g.id.as_str().into(),
                name: if g.ready {
                    g.name.as_str()
                } else {
                    "joining…"
                }
                .into(),
                initials: if g.ready {
                    initials(&g.name)
                } else {
                    "…".into()
                }
                .into(),
                unread: count(
                    s.unread
                        .iter()
                        .filter(|(k, _)| k.starts_with(&format!("{}/", g.id)))
                        .map(|(_, v)| *v)
                        .sum(),
                ),
                active: Some(&g.id) == s.group.as_ref(),
            })
            .collect();

        let channels: Vec<ChannelLine> = group
            .map(|g| {
                g.channels
                    .iter()
                    .map(|c| ChannelLine {
                        id: c.id.as_str().into(),
                        name: c.name.as_str().into(),
                        voice: c.kind == "voice",
                        private: c.private,
                        slowmode: i32::try_from(c.slowmode).unwrap_or(i32::MAX),
                        unread: count(*s.unread.get(&format!("{}/{}", g.id, c.id)).unwrap_or(&0)),
                        active: Some(&c.id) == s.channel.as_ref(),
                        here: self.in_room(s, &g.id, &c.id),
                        people: self.room_people(s, &g.id, &c.id),
                    })
                    .collect()
            })
            .unwrap_or_default();

        let messages: Vec<Line> = s
            .messages
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let head = i == 0 || {
                    let prev = &s.messages[i - 1];
                    prev.author != m.author || m.at.saturating_sub(prev.at) >= CONTINUES_SECS
                };
                Line {
                    id: m.id.as_str().into(),
                    name: Self::name_of(s, &m.author).into(),
                    time: time(m.at).into(),
                    text: m.text.as_str().into(),
                    mine: m.mine,
                    head,
                    can_remove: !m.mine && channel.is_some_and(|c| c.can_moderate),
                    chips: Self::chips(s, &m.reactions),
                    zap_sats: if m.zap_sats > 0 {
                        zaps::sats(m.zap_sats).into()
                    } else {
                        SharedString::new()
                    },
                    zap_note: format!(
                        "{} zap{} — counted from receipts signed by {}'s wallet",
                        m.zap_count,
                        if m.zap_count == 1 { "" } else { "s" },
                        Self::name_of(s, &m.author)
                    )
                    .into(),
                    files: m
                        .files
                        .iter()
                        .map(|f| Self::file_line(s, &f.hash, false, &f.name, f.size, &f.mime))
                        .collect(),
                }
            })
            .collect();

        // Online first (and me), then everyone else.
        let mut members: Vec<MemberUi> = s
            .members
            .iter()
            .map(|m| {
                let mut detail: Vec<String> = Vec::new();
                if m.anchor {
                    detail.push("anchor".into());
                }
                if m.owner {
                    detail.push("owner".into());
                } else if !m.roles.is_empty() {
                    detail.push(m.roles.join(", "));
                }
                if m.muted_until.is_some() {
                    detail.push("muted".into());
                }
                MemberUi {
                    npub: m.npub.as_str().into(),
                    name: Self::name_of(s, &m.npub).into(),
                    here: m.me || s.present.contains(&m.npub),
                    me: m.me,
                    detail: detail.join(" · ").into(),
                }
            })
            .collect();
        members.sort_by_key(|m| !m.here);

        let others = s.members.iter().filter(|m| !m.me).count();
        let here = s
            .members
            .iter()
            .filter(|m| !m.me && s.present.contains(&m.npub))
            .count();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let muted_for = group
            .and_then(|g| g.muted_until)
            .filter(|&u| u > now)
            .map(|u| (u - now).div_ceil(60));
        let reading = group.is_some_and(|g| g.reading);
        let refusal = match (channel, muted_for, newcomer_left(group)) {
            (None, ..) => "no channel".to_owned(),
            _ if reading => "join to post".into(),
            (Some(_), Some(min), _) => format!("muted for another {min} min"),
            (Some(_), None, Some(left)) => {
                format!("new here — you can post in {}:{:02}", left / 60, left % 60)
            }
            (Some(c), None, None) if !c.can_send => "you cannot post in this channel".into(),
            _ => String::new(),
        };
        let view = ViewUi {
            group_name: group
                .map(|g| if g.ready { g.name.as_str() } else { "joining…" })
                .unwrap_or_default()
                .into(),
            channel_name: channel.map(|c| c.name.as_str()).unwrap_or_default().into(),
            reach: if others == 0 {
                "just you so far".into()
            } else if here == 0 {
                "nobody online — kept here until someone is".into()
            } else {
                format!("{here} of {others} online").into()
            },
            reach_ok: others == 0 || here > 0,
            members: count(s.members.len()),
            // A public group's link is anyone's to pass on (M13).
            can_invite: group.is_some_and(|g| g.can_invite || g.public),
            refusal: refusal.into(),
            placeholder: channel
                .map(|c| format!("message #{}", c.name))
                .unwrap_or_default()
                .into(),
            banner: match group {
                Some(g) if g.removed && g.banned => "You were banned from this group. Your copy of the conversation up to then is still on this disk; nothing new will reach you.".into(),
                Some(g) if g.removed => "You are no longer a member of this group. Your copy of the conversation up to then is still on this disk; nothing new will reach you.".into(),
                _ => SharedString::new(),
            },
            joining: group.is_some_and(|g| !g.ready),
            reading,
            send_error: s.send_error.clone().unwrap_or_default().into(),
            slowmode: channel.map_or(0, |c| i32::try_from(c.slowmode).unwrap_or(i32::MAX)),
            max_chars: count(s.max_chars),
            my_name: Self::name_of(s, &s.me).into(),
            has_groups: !s.groups.is_empty(),
        };
        let settings = group.map_or_else(SettingsUi::default, |g| SettingsUi {
            name: g.name.as_str().into(),
            public: g.public,
            newcomer_wait_set: i32::try_from(g.newcomer_wait_set).unwrap_or(i32::MAX),
            newcomer_wait: i32::try_from(g.newcomer_wait).unwrap_or(i32::MAX),
            about: g.about.as_str().into(),
            me_owner: g.me_owner,
            can_rename: g.can_rename,
            can_manage: g.can_manage_channels,
            can_ban: g.can_ban,
            can_compact: g.can_compact,
            seq: i32::try_from(g.seq).unwrap_or(i32::MAX),
            has_any: g.ready
                && !g.removed
                && (g.me_owner
                    || g.can_rename
                    || g.can_manage_channels
                    || g.can_ban
                    || g.can_compact),
        });
        let bans: Vec<BanUi> = s
            .bans
            .iter()
            .map(|b| BanUi {
                npub: b.npub.as_str().into(),
                name: b
                    .name
                    .clone()
                    .unwrap_or_else(|| Self::name_of(s, &b.npub))
                    .into(),
            })
            .collect();
        let person = s
            .person
            .as_ref()
            .and_then(|p| s.members.iter().find(|m| &m.npub == p))
            .map(|m| PersonUi {
                npub: m.npub.as_str().into(),
                name: Self::name_of(s, &m.npub).into(),
                rank: if m.owner {
                    "Owner".into()
                } else if m.roles.is_empty() {
                    "Member".into()
                } else {
                    m.roles.join(", ").into()
                },
                muted_for: m
                    .muted_until
                    .filter(|&u| u > now)
                    .map_or(0, |u| i32::try_from((u - now).div_ceil(60)).unwrap_or(i32::MAX)),
                me: m.me,
                can_kick: m.can_kick,
                can_ban: m.can_ban,
                can_mute: m.can_mute,
                can_set_roles: m.can_set_roles,
            });
        let message_error: SharedString = s.message_error.clone().unwrap_or_default().into();
        let pending: Vec<PendingUi> = s
            .pending
            .iter()
            .map(|f| PendingUi {
                hash: f.hash.as_str().into(),
                name: f.name.as_str().into(),
                size: human_bytes(f.size).into(),
            })
            .collect();
        let dms = self.dm_render(s);
        let voice = self.voice_render(s);
        let key = format!(
            "{}/{}",
            s.group.as_deref().unwrap_or_default(),
            s.channel.as_deref().unwrap_or_default()
        );

        let _ = self.ui.upgrade_in_event_loop(move |ui| {
            LISTS.with(|l| {
                let l = l.get_or_init(|| {
                    let l = Lists::default();
                    ui.set_groups(ModelRc::from(l.groups.clone()));
                    ui.set_channels(ModelRc::from(l.channels.clone()));
                    ui.set_members(ModelRc::from(l.members.clone()));
                    ui.set_bans(ModelRc::from(l.bans.clone()));
                    l
                });
                patch(&l.groups, groups, PartialEq::eq);
                let channels: Vec<ChannelUi> =
                    channels.into_iter().map(ChannelLine::into_ui).collect();
                patch(&l.channels, channels, channel_same);
                patch(&l.members, members, PartialEq::eq);
                patch(&l.bans, bans, PartialEq::eq);
            });
            ui.set_view(view);
            ui.set_settings(settings);
            ui.set_message_error(message_error);
            ui.set_pending(ModelRc::new(VecModel::from(pending)));
            dms.apply(&ui);
            voice.apply(&ui);
            match person {
                Some(p) => ui.set_person(p),
                // Kicked or banned while their dialog was open: it closes.
                None if ui.get_dialog() == "member" => ui.set_dialog("".into()),
                None => {}
            }
            let messages: Vec<MessageUi> = messages.into_iter().map(Line::into_row).collect();
            MESSAGES.with(|cell| {
                let mut cell = cell.borrow_mut();
                match &*cell {
                    Some((k, model)) if *k == key => patch(model, messages, same),
                    _ => {
                        let model = Rc::new(VecModel::from(messages));
                        ui.set_messages(ModelRc::from(model.clone()));
                        *cell = Some((key, model));
                        ui.invoke_channel_opened();
                    }
                }
            });
        });
    }
}

impl Chat {
    /// A message's reactions, one chip per emoji, in first-seen order.
    fn chips(s: &State, rs: &[ReactionView]) -> Vec<ChipUi> {
        let mut out: Vec<(String, ChipUi, Vec<String>)> = Vec::new();
        for r in rs {
            let i = match out.iter().position(|(e, ..)| *e == r.emoji) {
                Some(i) => i,
                None => {
                    out.push((r.emoji.clone(), ChipUi::default(), Vec::new()));
                    out.len() - 1
                }
            };
            let (_, chip, who) = &mut out[i];
            chip.count += 1;
            chip.mine |= r.mine;
            who.push(Self::name_of(s, &r.author));
        }
        out.into_iter()
            .map(|(emoji, chip, who)| ChipUi {
                emoji: emoji.into(),
                who: who.join(", ").into(),
                ..chip
            })
            .collect()
    }
}

/// A message row before it reaches Slint's thread: a `MessageUi` holds its
/// reactions in a `ModelRc`, which may not cross threads.
struct Line {
    id: SharedString,
    name: SharedString,
    time: SharedString,
    text: SharedString,
    mine: bool,
    head: bool,
    can_remove: bool,
    chips: Vec<ChipUi>,
    files: Vec<FileLine>,
    zap_sats: SharedString,
    zap_note: SharedString,
}

/// A channel row before it reaches Slint's thread: its voice people are a
/// `ModelRc` there.
struct ChannelLine {
    id: SharedString,
    name: SharedString,
    voice: bool,
    private: bool,
    slowmode: i32,
    unread: i32,
    active: bool,
    here: bool,
    people: Vec<crate::ui::VoicePersonUi>,
}

impl ChannelLine {
    fn into_ui(self) -> ChannelUi {
        ChannelUi {
            id: self.id,
            name: self.name,
            voice: self.voice,
            private: self.private,
            slowmode: self.slowmode,
            unread: self.unread,
            active: self.active,
            here: self.here,
            people: ModelRc::new(VecModel::from(self.people)),
        }
    }
}

fn channel_same(a: &ChannelUi, b: &ChannelUi) -> bool {
    a.id == b.id
        && a.name == b.name
        && a.voice == b.voice
        && a.private == b.private
        && a.slowmode == b.slowmode
        && a.unread == b.unread
        && a.active == b.active
        && a.here == b.here
        && a.people.iter().eq(b.people.iter())
}

/// A file before it reaches Slint's thread: its pixels become an `Image`
/// there.
struct FileLine {
    key: SharedString,
    dm: bool,
    name: SharedString,
    size: SharedString,
    image: bool,
    note: SharedString,
    pixels: Option<Pixels>,
}

impl FileLine {
    fn into_ui(self) -> FileUi {
        FileUi {
            key: self.key,
            dm: self.dm,
            name: self.name,
            size: self.size,
            image: self.image,
            shown: self.pixels.is_some(),
            picture: self
                .pixels
                .map(slint::Image::from_rgba8)
                .unwrap_or_default(),
            note: self.note,
        }
    }
}

impl Line {
    fn into_row(self) -> MessageUi {
        MessageUi {
            id: self.id,
            name: self.name,
            time: self.time,
            text: self.text,
            mine: self.mine,
            head: self.head,
            can_remove: self.can_remove,
            zap_sats: self.zap_sats,
            zap_note: self.zap_note,
            reactions: ModelRc::new(VecModel::from(self.chips)),
            files: ModelRc::new(VecModel::from(
                self.files.into_iter().map(FileLine::into_ui).collect::<Vec<_>>(),
            )),
        }
    }
}

/// The window's other lists, made once and patched ever after: a new model
/// rebuilds every row, and a row rebuilt under the keyboard takes the focus
/// with it — nothing has it then, and the window gets no keys.
#[derive(Default)]
struct Lists {
    groups: Rc<VecModel<GroupUi>>,
    channels: Rc<VecModel<ChannelUi>>,
    members: Rc<VecModel<MemberUi>>,
    bans: Rc<VecModel<BanUi>>,
}

thread_local! {
    /// The message rows on screen, and which channel they are — on Slint's
    /// thread only.
    static MESSAGES: RefCell<Option<(String, Rc<VecModel<MessageUi>>)>> = const { RefCell::new(None) };
    static LISTS: std::cell::OnceCell<Lists> = const { std::cell::OnceCell::new() };
}

/// Makes `model` equal `rows`, touching only the rows that differ.
fn patch<T: Clone + 'static>(model: &VecModel<T>, rows: Vec<T>, same: impl Fn(&T, &T) -> bool) {
    let keep = rows.len();
    // A row gone from the middle shifts every later one, and the ListView
    // keeps the heights of the rows that were there: blank gaps, new
    // messages far below. Removal rebuilds the list instead.
    if keep < model.row_count() {
        model.set_vec(rows);
        return;
    }
    for (i, row) in rows.into_iter().enumerate() {
        if i < model.row_count() {
            if !model.row_data(i).is_some_and(|old| same(&old, &row)) {
                model.set_row_data(i, row);
            }
        } else {
            model.push(row);
        }
    }
    while model.row_count() > keep {
        model.remove(model.row_count() - 1);
    }
}

/// Two rows show the same thing. A `ModelRc` compares by pointer, so the
/// reactions are compared by what is in them.
fn same(a: &MessageUi, b: &MessageUi) -> bool {
    let chips = |m: &MessageUi| m.reactions.iter().collect::<Vec<_>>();
    a.id == b.id
        && a.name == b.name
        && a.time == b.time
        && a.text == b.text
        && a.mine == b.mine
        && a.head == b.head
        && a.can_remove == b.can_remove
        && a.zap_sats == b.zap_sats
        && chips(a) == chips(b)
        && files_same(&a.files, &b.files)
}

/// Two lists of files show the same thing: a picture compares by whether
/// it is shown, not by its pixels.
fn files_same(a: &ModelRc<FileUi>, b: &ModelRc<FileUi>) -> bool {
    a.row_count() == b.row_count()
        && a.iter().zip(b.iter()).all(|(x, y)| {
            x.key == y.key && x.shown == y.shown && x.note == y.note && x.size == y.size
        })
}

fn is_picture(mime: &str) -> bool {
    mime.starts_with("image/") && mime != "image/svg+xml"
}

/// "1.2 MB", as the web window wrote it.
fn human_bytes(n: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let f = n as f64;
    if n < 1000 {
        format!("{n} B")
    } else if n < 1_000_000 {
        format!("{:.0} kB", f / 1e3)
    } else if n < 1_000_000_000 {
        format!("{:.1} MB", f / 1e6)
    } else {
        format!("{:.1} GB", f / 1e9)
    }
}

/// The channel a group opens on: its first text channel — a voice channel
/// has no messages to show, and opening one is joining a call.
fn first_text(g: Option<&GroupView>) -> Option<String> {
    g?.channels
        .iter()
        .find(|c| c.kind == "text")
        .map(|c| c.id.clone())
}

/// A group's id is a key, unreadable by design: the rail shows initials.
fn initials(name: &str) -> String {
    let s: String = name
        .split_whitespace()
        .take(2)
        .filter_map(|w| w.chars().next())
        .flat_map(char::to_uppercase)
        .collect();
    if s.is_empty() { "?".into() } else { s }
}

fn short_key(npub: &str) -> String {
    if npub.chars().count() > 20 {
        let head: String = npub.chars().take(10).collect();
        let tail: String = npub
            .chars()
            .rev()
            .take(4)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        format!("{head}…{tail}")
    } else {
        npub.to_owned()
    }
}

/// Today: the time; before: the date too.
fn time(at: u64) -> String {
    use chrono::{Local, TimeZone};
    let Some(t) = i64::try_from(at)
        .ok()
        .and_then(|s| Local.timestamp_opt(s, 0).single())
    else {
        return String::new();
    };
    if t.date_naive() == Local::now().date_naive() {
        t.format("%H:%M").to_string()
    } else {
        t.format("%b %-d, %H:%M").to_string()
    }
}

fn count(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_and_short_keys() {
        assert_eq!(initials("rust friends club"), "RF");
        assert_eq!(initials("   "), "?");
        assert_eq!(
            short_key("npub1abcdefghijklmnopqrstuvwxyz"),
            "npub1abcde…wxyz"
        );
        assert_eq!(short_key("short"), "short");
    }
}

/// "Bob is here", "Bob and Carol are here", "Bob, Carol and 3 others you
/// know are here" — the friend signal on a browse row (M13).
fn known_line(names: &[String], count: usize) -> String {
    let others = count.saturating_sub(names.len());
    let Some((last, rest)) = names.split_last() else {
        return String::new();
    };
    match (rest, others) {
        ([], 0) => format!("{last} is here"),
        (rest, 0) => format!("{} and {last} are here", rest.join(", ")),
        (_, n) => format!(
            "{} and {n} other{} you know are here",
            names.join(", "),
            if n == 1 { "" } else { "s" }
        ),
    }
}

#[cfg(test)]
mod known_tests {
    use super::known_line;

    #[test]
    fn the_friend_line_reads_right() {
        let n = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(known_line(&[], 0), "");
        assert_eq!(known_line(&n(&["Bob"]), 1), "Bob is here");
        assert_eq!(known_line(&n(&["Bob", "Carol"]), 2), "Bob and Carol are here");
        assert_eq!(known_line(&n(&["A", "B", "C"]), 3), "A, B and C are here");
        assert_eq!(known_line(&n(&["Bob"]), 2), "Bob and 1 other you know are here");
        assert_eq!(known_line(&n(&["A", "B", "C"]), 9), "A, B, C and 6 others you know are here");
    }
}
