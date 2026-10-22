//! Voice (M12-7) — `voice.svelte.ts`: who is in which voice channel, this
//! person's call, mute, deafen, push-to-talk, a 1:1 call from a DM, the
//! call ringing here, and the audio settings. hit-app-core owns the call;
//! this mirrors its `VoiceView` (pumped once a second, and on change) and
//! sends what the person presses.
//!
//! Audio opens only on a join, a call or an answer — never at start — and
//! with the devices the settings name, or `HIT_AUDIO_IN` / `HIT_AUDIO_OUT`.

use std::future::Future;

use hit_app_core::state::{AppError, VoiceView};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{Chat, State};
use crate::ui::{CallUi, DeviceUi, MainWindow, VoicePersonUi};

/// Samples of one 20 ms frame at 48 kHz: a run of dropped samples is one
/// skip, as the web window counted.
const FRAME: u64 = 960;

impl Chat {
    pub(super) async fn voice_changed(&self, s: &mut State, v: VoiceView) {
        let ended = s.voice.as_ref().is_some_and(|o| o.call.is_some()) && v.call.is_none();
        if v.call.is_none() {
            s.talking = false;
        }
        let mut people: Vec<String> = v.rooms.values().flat_map(|r| r.values().flatten().cloned()).collect();
        people.extend(v.incoming.clone());
        if let Some(w) = v.call.as_ref().and_then(|c| c.with.clone()) {
            people.push(w);
        }
        s.voice = Some(v);
        self.learn_names(s, people, false).await;
        if ended {
            // The bar that had the keyboard is gone: the composer takes it.
            let _ = self.ui.upgrade_in_event_loop(|ui| ui.invoke_refocus());
        }
    }

    /// Joins a voice channel of the open group.
    pub async fn join_voice(&self, channel: String) {
        let Some(g) = self.s.lock().await.group.clone() else { return };
        self.voice_step(async move |app| app.voice_join(&g, &channel).await)
            .await;
    }

    /// Rings `npub` (a 1:1 call from a DM).
    pub async fn ring(&self, npub: String) {
        self.voice_step(async move |app| app.voice_call(&npub).await).await;
    }

    pub async fn answer(&self) {
        self.voice_step(async move |app| app.voice_answer().await).await;
    }

    pub async fn decline(&self) {
        self.voice_step(async move |app| app.voice_decline().await).await;
    }

    pub async fn leave_call(&self) {
        self.voice_step(async move |app| app.voice_leave().await).await;
    }

    /// Mute, deafen or push-to-talk, flipped.
    pub async fn toggle(&self, what: Toggle) {
        let call = {
            let s = self.s.lock().await;
            s.voice.as_ref().and_then(|v| v.call.clone())
        };
        let Some(c) = call else { return };
        let (m, d, p) = match what {
            Toggle::Mute => (Some(!c.muted), None, None),
            Toggle::Deafen => (None, Some(!c.deafened), None),
            Toggle::PushToTalk => (None, None, Some(!c.push_to_talk)),
        };
        self.voice_step(async move |app| app.voice_set(m, d, p).await)
            .await;
    }

    /// The talk key or button, held or let go.
    pub async fn hold(&self, down: bool) {
        let mut s = self.s.lock().await;
        if s.talking == down {
            return;
        }
        s.talking = down;
        let ptt = s
            .voice
            .as_ref()
            .and_then(|v| v.call.as_ref())
            .is_some_and(|c| c.push_to_talk);
        if ptt || !down {
            let _ = self.app.voice_talk(down).await;
        }
        self.render(&mut s);
    }

    /// One voice action: busy while the audio opens, its refusal shown,
    /// the view read again either way.
    async fn voice_step<F, Fut>(&self, step: F)
    where
        F: FnOnce(std::sync::Arc<hit_app_core::state::App>) -> Fut,
        Fut: Future<Output = Result<(), AppError>>,
    {
        {
            let mut s = self.s.lock().await;
            s.call_busy = true;
            s.voice_error = None;
            self.render(&mut s);
        }
        let outcome = step(self.app.clone()).await;
        let view = self.app.voice_view().await;
        let mut s = self.s.lock().await;
        s.call_busy = false;
        s.voice_error = outcome.err().map(|e| e.to_string());
        if let Ok(v) = view {
            self.voice_changed(&mut s, v).await;
        }
        self.render(&mut s);
    }

    /// The audio dialog is opening: the devices there are, and the choice.
    pub async fn audio_settings(&self) -> Result<AudioChoice, String> {
        let a = self.app.audio_settings().await.map_err(|e| e.to_string())?;
        let dev = |d: &Vec<_>| -> Vec<DeviceUi> {
            d.iter()
                .map(|x: &hit_app_core::state::DeviceView| DeviceUi {
                    id: x.id.as_str().into(),
                    name: format!("{} ({})", x.name, x.id).into(),
                })
                .collect()
        };
        Ok(AudioChoice {
            inputs: dev(&a.devices.inputs),
            outputs: dev(&a.devices.outputs),
            input: a.input.unwrap_or_default().into(),
            output: a.output.unwrap_or_default().into(),
            echo: a.echo_cancel,
        })
    }

    pub fn save_audio(&self, input: String, output: String, echo: bool) -> Result<(), String> {
        let some = |x: String| Some(x).filter(|x| !x.is_empty());
        self.app
            .set_audio(some(input), some(output), echo)
            .map_err(|e| e.to_string())
    }

    pub(super) fn in_room(&self, s: &State, group: &str, channel: &str) -> bool {
        s.voice
            .as_ref()
            .and_then(|v| v.call.as_ref())
            .is_some_and(|c| c.group.as_deref() == Some(group) && c.channel.as_deref() == Some(channel))
    }

    /// Who is in a voice channel, and who of them is speaking.
    pub(super) fn room_people(&self, s: &State, group: &str, channel: &str) -> Vec<VoicePersonUi> {
        let Some(v) = &s.voice else { return Vec::new() };
        let here = self.in_room(s, group, channel);
        v.rooms
            .get(group)
            .and_then(|r| r.get(channel))
            .map(|people| {
                people
                    .iter()
                    .map(|p| VoicePersonUi {
                        name: Self::name_of(s, p).into(),
                        speaking: here && Self::speaking(s, v, p),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn speaking(s: &State, v: &VoiceView, npub: &str) -> bool {
        let Some(c) = &v.call else { return false };
        if npub == s.me {
            // Heard: speaking, not muted, allowed, and the key held if
            // push-to-talk.
            return c.me_speaking
                && !c.muted
                && c.can_speak
                && (!c.push_to_talk || s.talking);
        }
        c.people.iter().any(|p| p.npub == npub && p.speaking)
    }

    pub(super) fn voice_render(&self, s: &State) -> VoiceRender {
        let v = s.voice.as_ref();
        let call = v.and_then(|v| v.call.as_ref()).map_or_else(CallUi::default, |c| {
            let connected = c.people.iter().filter(|p| p.connected).count();
            let waiting = c.people.len() - connected;
            let (title, sub) = match &c.with {
                Some(w) => (
                    Self::name_of(s, w),
                    if c.calling {
                        "ringing…".to_owned()
                    } else if connected > 0 {
                        format!(
                            "{} · direct, end to end",
                            if c.people.first().is_some_and(|p| p.speaking) { "speaking" } else { "connected" }
                        )
                    } else {
                        "connecting…".into()
                    },
                ),
                None => {
                    let g = s.groups.iter().find(|g| Some(&g.id) == c.group.as_ref());
                    let channel = g
                        .and_then(|g| g.channels.iter().find(|x| Some(&x.id) == c.channel.as_ref()))
                        .map_or_else(|| c.channel.clone().unwrap_or_default(), |x| x.name.clone());
                    let who = if c.people.is_empty() {
                        "nobody else here yet".to_owned()
                    } else if waiting > 0 {
                        format!("{connected} connected, {waiting} connecting")
                    } else {
                        format!("{connected} connected")
                    };
                    (
                        channel,
                        format!("{} · {who}", g.map(|g| g.name.as_str()).unwrap_or_default()),
                    )
                }
            };
            let glitches = v.and_then(|v| v.audio.as_ref()).map_or(0, |a| {
                (a.mic_dropped + a.speaker_dropped).div_ceil(FRAME) + a.mic_short + a.speaker_underruns
            });
            CallUi {
                active: true,
                dm: c.with.is_some(),
                title: title.into(),
                sub: sub.into(),
                connected: connected > 0,
                muted: c.muted,
                deafened: c.deafened,
                push_to_talk: c.push_to_talk,
                can_speak: c.can_speak,
                talking: s.talking,
                glitches: i32::try_from(glitches).unwrap_or(i32::MAX),
                with: c.with.clone().unwrap_or_default().into(),
            }
        });
        VoiceRender {
            call,
            busy: s.call_busy,
            error: s.voice_error.clone().unwrap_or_default().into(),
            incoming: v
                .and_then(|v| v.incoming.as_ref())
                .map(|p| Self::name_of(s, p))
                .unwrap_or_default()
                .into(),
        }
    }

    /// A call is ringing here: the window has no sound of its own, so the
    /// desktop is asked to draw the eye to it (the taskbar, an urgent
    /// workspace).
    pub(super) fn ask_attention(ui: &slint::Weak<MainWindow>) {
        let _ = ui.upgrade_in_event_loop(|ui| {
            use slint::winit_030::{WinitWindowAccessor, winit::window::UserAttentionType};
            ui.window().with_winit_window(|w| {
                w.request_user_attention(Some(UserAttentionType::Critical));
            });
        });
    }
}

#[derive(Clone, Copy)]
pub enum Toggle {
    Mute,
    Deafen,
    PushToTalk,
}

pub struct AudioChoice {
    pub inputs: Vec<DeviceUi>,
    pub outputs: Vec<DeviceUi>,
    pub input: SharedString,
    pub output: SharedString,
    pub echo: bool,
}

pub(super) struct VoiceRender {
    call: CallUi,
    busy: bool,
    error: SharedString,
    incoming: SharedString,
}

impl VoiceRender {
    pub(super) fn apply(self, ui: &MainWindow) {
        ui.set_call(self.call);
        ui.set_call_busy(self.busy);
        ui.set_voice_error(self.error);
        ui.set_incoming(self.incoming);
    }
}

impl AudioChoice {
    /// Into the dialog's properties, on Slint's thread.
    pub fn apply(self, ui: &MainWindow) {
        ui.set_audio_inputs(ModelRc::new(VecModel::from(self.inputs)));
        ui.set_audio_outputs(ModelRc::new(VecModel::from(self.outputs)));
        ui.set_audio_input(self.input);
        ui.set_audio_output(self.output);
        ui.set_audio_echo(self.echo);
        ui.set_audio_loaded(true);
    }
}
