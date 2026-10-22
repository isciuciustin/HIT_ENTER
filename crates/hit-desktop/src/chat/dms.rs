//! Direct messages (M12-6) — the DM half of `chat.svelte.ts`: the
//! conversations, the open one, sending text and files, first contact by
//! link, and DM unread counts (`dm/<npub>` read marks).

use std::{cell::RefCell, collections::BTreeMap, path::PathBuf, rc::Rc};

use hit_app_core::state::{DmView, MAX_DM_CHARS};
use slint::{ModelRc, SharedString, VecModel};

use super::{Chat, FileLine, State, count, patch, short_key, time};
use crate::ui::{ConvUi, DmUi, MainWindow, ViewUi};

impl Chat {
    /// The DM pane: the open conversation, or the newest.
    pub async fn show_dms(&self) {
        let mut s = self.s.lock().await;
        s.dms_open = true;
        if s.dm_with.is_none() {
            s.dm_with = s.dms.iter().max_by_key(|d| d.at).map(|d| d.with.clone());
        }
        self.mark_dm_read(&mut s).await;
        self.render(&mut s);
    }

    /// Opens the conversation with `npub`, starting one if there is none.
    pub async fn open_dm(&self, npub: String) {
        let mut s = self.s.lock().await;
        s.dms_open = true;
        s.dm_error = None;
        if !s.names.contains_key(&npub) {
            self.learn_names(&mut s, vec![npub.clone()], false).await;
        }
        s.dm_with = Some(npub);
        self.mark_dm_read(&mut s).await;
        self.render(&mut s);
    }

    /// Sends `text` in the open conversation; whether it went.
    pub async fn send_dm(&self, text: String) -> bool {
        let mut s = self.s.lock().await;
        let Some(to) = s.dm_with.clone() else {
            return false;
        };
        s.dm_error = None;
        let sent = match self.app.send_dm(&to, &text).await {
            Ok(d) => {
                self.dm_arrived(&mut s, d).await;
                true
            }
            Err(e) => {
                s.dm_error = Some(e.to_string());
                false
            }
        };
        self.render(&mut s);
        sent
    }

    /// Sends files in the open conversation, each as its own message,
    /// encrypted on this side (NIP-17 kind 15).
    pub async fn send_dm_files(&self, paths: Vec<PathBuf>) {
        let mut s = self.s.lock().await;
        let Some(to) = s.dm_with.clone() else { return };
        s.dm_error = None;
        for p in paths {
            match self.app.send_dm_file(&to, &p).await {
                Ok(d) => self.dm_arrived(&mut s, d).await,
                Err(e) => {
                    s.dm_error = Some(e.to_string());
                    break;
                }
            }
        }
        self.render(&mut s);
    }

    /// First contact: someone's `hit-enter:contact?…` link. Their app
    /// answers once; then the conversation opens.
    pub async fn add_contact(&self, link: String) -> Result<(), String> {
        let who = self
            .app
            .add_contact(link.trim())
            .await
            .map_err(|e| e.to_string())?;
        self.open_dm(who).await;
        Ok(())
    }

    pub(super) async fn load_dms(&self, s: &mut State) {
        s.dms = self.app.dms().await.unwrap_or_default();
        s.dm_unread = self
            .app
            .dm_unread()
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
        let people: Vec<String> = s.dms.iter().map(|d| d.with.clone()).collect();
        self.learn_names(s, people, false).await;
    }

    pub(super) async fn dm_arrived(&self, s: &mut State, d: DmView) {
        if s.dms.iter().any(|x| x.id == d.id) {
            return;
        }
        if !s.names.contains_key(&d.with) {
            self.learn_names(s, vec![d.with.clone()], false).await;
        }
        let (mine, with) = (d.mine, d.with.clone());
        s.dms.push(d);
        let n = s.dms.len();
        if n > 1 && s.dms[n - 2].at > s.dms[n - 1].at {
            s.dms.sort_by_key(|x| x.at);
        }
        if mine {
            return;
        }
        if s.dms_open && !s.unfocused && s.dm_with.as_deref() == Some(with.as_str()) {
            self.mark_dm_read(s).await;
        } else {
            *s.dm_unread.entry(with).or_default() += 1;
        }
    }

    /// The open conversation is read up to its newest message.
    pub(super) async fn mark_dm_read(&self, s: &mut State) {
        let Some(w) = s.dm_with.clone().filter(|_| s.dms_open && !s.unfocused) else {
            return;
        };
        s.dm_unread.remove(&w);
        if let Some(last) = s.dms.iter().rfind(|d| d.with == w) {
            let _ = self.app.mark_read("dm", &w, last.at).await;
        }
    }

    /// The DM pane's rows, ready to cross to Slint's thread.
    pub(super) fn dm_render(&self, s: &State) -> DmRender {
        // Newest conversation first.
        let mut last: BTreeMap<&str, &DmView> = BTreeMap::new();
        for d in &s.dms {
            last.insert(d.with.as_str(), d);
        }
        let mut convs: Vec<&DmView> = last.into_values().collect();
        convs.sort_by_key(|d| std::cmp::Reverse(d.at));
        // Someone just opened from a member list has no messages yet.
        let fresh = s
            .dm_with
            .as_ref()
            .filter(|w| !s.dms.iter().any(|d| &d.with == *w));
        let mut rows: Vec<ConvUi> = fresh
            .map(|w| ConvUi {
                with: w.as_str().into(),
                name: Self::name_of(s, w).into(),
                last: SharedString::new(),
                unread: 0,
                active: true,
            })
            .into_iter()
            .collect();
        rows.extend(convs.into_iter().map(|d| ConvUi {
            with: d.with.as_str().into(),
            name: Self::name_of(s, &d.with).into(),
            last: format!(
                "{}{}",
                if d.mine { "you: " } else { "" },
                d.file
                    .as_ref()
                    .map_or_else(|| d.text.clone(), |f| format!("file: {}", f.name))
            )
            .into(),
            unread: count(*s.dm_unread.get(&d.with).unwrap_or(&0)),
            active: Some(&d.with) == s.dm_with.as_ref(),
        }));

        let with = s.dm_with.clone().unwrap_or_default();
        let thread: Vec<DmLine> = s
            .dms
            .iter()
            .filter(|d| d.with == with)
            .map(|d| DmLine {
                id: d.id.as_str().into(),
                mine: d.mine,
                text: d.text.as_str().into(),
                time: time(d.at).into(),
                file: d
                    .file
                    .as_ref()
                    .map(|f| Self::file_line(s, &d.id, true, &f.name, f.size, &f.mime)),
            })
            .collect();
        let name = if with.is_empty() {
            String::new()
        } else {
            Self::name_of(s, &with)
        };
        DmRender {
            open: s.dms_open,
            unread: count(s.dm_unread.values().sum()),
            convs: rows,
            thread,
            view: ViewUi {
                placeholder: format!("message {name}").into(),
                send_error: s.dm_error.clone().unwrap_or_default().into(),
                max_chars: count(MAX_DM_CHARS),
                ..ViewUi::default()
            },
            with_key: if with.is_empty() { String::new() } else { short_key(&with) }.into(),
            with_name: name.into(),
            with: with.into(),
        }
    }
}

/// A DM before it reaches Slint's thread.
pub(super) struct DmLine {
    id: SharedString,
    mine: bool,
    text: SharedString,
    time: SharedString,
    file: Option<FileLine>,
}

pub(super) struct DmRender {
    open: bool,
    unread: i32,
    convs: Vec<ConvUi>,
    thread: Vec<DmLine>,
    view: ViewUi,
    with: SharedString,
    with_name: SharedString,
    with_key: SharedString,
}

thread_local! {
    /// The open conversation's rows, and whose they are — patched, as the
    /// messages are, so focus and scroll survive a new message.
    static THREAD: RefCell<Option<(SharedString, Rc<VecModel<DmUi>>)>> = const { RefCell::new(None) };
    static CONVS: RefCell<Option<Rc<VecModel<ConvUi>>>> = const { RefCell::new(None) };
}

impl DmRender {
    pub(super) fn apply(self, ui: &MainWindow) {
        ui.set_dms_open(self.open);
        ui.set_dm_unread(self.unread);
        ui.set_dm_view(self.view);
        ui.set_dm_with_name(self.with_name);
        ui.set_dm_with_key(self.with_key);
        ui.set_dm_with(self.with.clone());
        CONVS.with(|c| {
            let mut c = c.borrow_mut();
            let model = c.get_or_insert_with(|| {
                let m = Rc::new(VecModel::default());
                ui.set_convs(ModelRc::from(m.clone()));
                m
            });
            patch(model, self.convs, PartialEq::eq);
        });
        let rows: Vec<DmUi> = self
            .thread
            .into_iter()
            .map(|d| DmUi {
                id: d.id,
                mine: d.mine,
                text: d.text,
                time: d.time,
                has_file: d.file.is_some(),
                file: d.file.map(FileLine::into_ui).unwrap_or_default(),
            })
            .collect();
        let with = self.with;
        THREAD.with(|cell| {
            let mut cell = cell.borrow_mut();
            match &*cell {
                Some((k, model)) if *k == with => patch(model, rows, dm_same),
                _ => {
                    let model = Rc::new(VecModel::from(rows));
                    ui.set_thread(ModelRc::from(model.clone()));
                    *cell = Some((with, model));
                    ui.invoke_dm_opened();
                }
            }
        });
    }
}

/// A picture compares by whether it is shown, not by its pixels.
fn dm_same(a: &DmUi, b: &DmUi) -> bool {
    a.id == b.id
        && a.text == b.text
        && a.time == b.time
        && a.file.shown == b.file.shown
        && a.file.note == b.file.note
}
