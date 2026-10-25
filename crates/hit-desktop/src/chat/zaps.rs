//! Zaps and the profile (M12-8) — `ZapDialog.svelte` and
//! `ProfileDialog.svelte`. HIT_ENTER never holds money: a zap goes from the
//! person's wallet to the recipient's, and the 1% fee is a second payment on
//! top, after it, shown before anything is paid (`DESIGN.md` §11).
//!
//! The NWC connection string is typed into a password field and handed to
//! hit-app-core, which keeps it in the keychain; it is never shown, logged
//! or kept here.

use std::path::Path;

use hit_app_core::state::{InvoiceView, ZapQuoteView};
use slint::{Rgba8Pixel, SharedPixelBuffer};

use super::{Chat, State};
use crate::ui::ProfileUi;

/// "1,000".
pub fn sats(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// The fee's line after a wallet zap: paid separately, after it.
fn fee_note(fee: u64) -> String {
    if fee > 0 {
        format!(" The {}-sat fee is being paid separately.", sats(fee))
    } else {
        String::new()
    }
}

/// A zap paid from the wallet: what to say now, and whether it is counted
/// yet — when not, [`Chat::wallet_zap_counted`] waits for it.
pub struct WalletZap {
    pub result: String,
    pub counted: bool,
    pub fee: u64,
}

/// What the zap dialog shows of a quote, before anything is paid.
pub struct QuoteText {
    pub quote: String,
    pub fee_note: String,
    pub privacy: String,
    pub total: String,
    pub wallet: bool,
}

/// An invoice for the QR step: its text and its code, as pixels.
pub struct Invoice {
    pub bolt11: String,
    pub sats: String,
    pub qr: SharedPixelBuffer<Rgba8Pixel>,
}

impl Chat {
    /// Everything the profile dialog shows.
    pub async fn profile(&self) -> ProfileUi {
        let st = self.app.status().await.ok();
        let wallet = self.app.wallet_view().await.ok();
        let inbox = self.app.inbox().await.ok();
        let contact = self.app.contact_link().await.unwrap_or_default();
        let mut p = ProfileUi {
            loaded: wallet.is_some(),
            contact: contact.into(),
            ..ProfileUi::default()
        };
        if let Some(st) = st {
            p.name = st.name.unwrap_or_default().into();
            p.npub = st.npub.unwrap_or_default().into();
            p.device = st.device.unwrap_or_default().into();
            p.relays = st.relays;
            p.min_password = i32::try_from(st.min_password).unwrap_or(i32::MAX);
        }
        p.publishes = self.app.publishes();
        p.public_relays = self.app.public_relays().join(", ").into();
        p.lists_groups = self.app.lists_groups();
        if let Some(w) = wallet {
            p.address = w.address.unwrap_or_default().into();
            p.zappable = w.zappable;
            p.connected = w.connected;
            p.wallet_relays = w.relays.join(", ").into();
            p.fee_on = w.fee_on;
            p.fee_percent = i32::try_from(w.fee_percent).unwrap_or(0);
            p.fee_min = i32::try_from(w.fee_min_sats).unwrap_or(0);
        }
        if let Some(i) = inbox {
            p.inbox_loaded = true;
            p.inbox_enabled = i.enabled;
            p.inbox_available = i32::try_from(i.available.len()).unwrap_or(0);
        }
        p
    }

    pub async fn set_my_name(&self, name: String) -> Result<String, String> {
        self.app.set_name(&name).await.map_err(|e| e.to_string())?;
        let mut s = self.s.lock().await;
        let me = s.me.clone();
        s.names.insert(me, name.trim().to_owned());
        self.render(&mut s);
        Ok("Name saved. Members see it at their next sync.".into())
    }

    pub async fn set_address(&self, address: String) -> Result<String, String> {
        self.app
            .set_lightning_address(address.trim())
            .await
            .map_err(|e| e.to_string())?;
        Ok(if address.trim().is_empty() {
            "Lightning address removed.".into()
        } else {
            "Lightning address saved; members see it at their next sync.".into()
        })
    }

    pub async fn connect_wallet(&self, connection: String) -> Result<String, String> {
        self.app
            .connect_wallet(connection.trim())
            .await
            .map_err(|e| e.to_string())?;
        Ok("Wallet connected. The connection string is in your keychain.".into())
    }

    pub async fn disconnect_wallet(&self) -> Result<String, String> {
        self.app.disconnect_wallet().await.map_err(|e| e.to_string())?;
        Ok("Wallet disconnected; revoke it in your wallet too.".into())
    }

    pub async fn toggle_inbox(&self) -> Result<String, String> {
        let on = self.app.inbox().await.map_err(|e| e.to_string())?.enabled;
        let now = self.app.set_inbox(!on).await.map_err(|e| e.to_string())?;
        Ok(if now.enabled {
            "Your groups' anchors will keep your DMs while you are away, for 7 days.".into()
        } else {
            "Your DMs now wait only on the sender's device until you are back.".into()
        })
    }

    pub async fn set_public_relays(&self, relays: String) -> Result<String, String> {
        let list: Vec<String> = relays.split([',', ' ']).map(str::to_owned).collect();
        let now = self
            .app
            .set_public_relays(&list)
            .await
            .map_err(|e| e.to_string())?;
        Ok(match now.len() {
            0 => "Your profile goes to no public relay now.".into(),
            1 => "Your profile goes to 1 public relay.".into(),
            n => format!("Your profile goes to {n} public relays."),
        })
    }

    pub async fn toggle_list_groups(&self) -> Result<String, String> {
        let on = !self.app.lists_groups();
        self.app
            .set_list_groups(on)
            .await
            .map_err(|e| e.to_string())?;
        Ok(if on {
            "Your public groups are listed for people you know.".into()
        } else {
            "Your public groups are no longer listed.".into()
        })
    }

    pub async fn backup_again(&self, path: &Path, password: String) -> Result<String, String> {
        self.app
            .backup_to_file(path, &password)
            .await
            .map_err(|e| e.to_string())?;
        Ok("Backup written.".into())
    }

    /// The zap dialog opens for message `id`: whom it pays.
    pub async fn open_zap(&self, id: String) -> Option<String> {
        let mut s = self.s.lock().await;
        let m = s.messages.iter().find(|m| m.id == id)?;
        let (group, who) = (m.group.clone(), Self::name_of(&s, &m.author));
        s.zap_target = Some((group, id));
        Some(who)
    }

    async fn target(&self) -> Result<(String, String, String), String> {
        let s = self.s.lock().await;
        let (g, id) = s.zap_target.clone().ok_or("no message to zap")?;
        let who = s
            .messages
            .iter()
            .find(|m| m.id == id)
            .map(|m| Self::name_of(&s, &m.author))
            .unwrap_or_default();
        Ok((g, id, who))
    }

    /// What `amount` costs: asked of the recipient's wallet's server.
    pub async fn zap_quote(&self, amount: u64) -> Result<QuoteText, String> {
        let (g, id, who) = self.target().await?;
        let q: ZapQuoteView = self
            .app
            .zap_quote(&g, &id, amount)
            .await
            .map_err(|e| e.to_string())?;
        Ok(QuoteText {
            quote: if q.fee > 0 {
                format!(
                    "{} sats to {who} · {} sats HIT_ENTER fee · {} total",
                    sats(q.amount),
                    sats(q.fee),
                    sats(q.total)
                )
            } else {
                format!("{} sats to {who}", sats(q.amount))
            },
            fee_note: if q.fee > 0 {
                format!(
                    "{who} gets exactly {}. The fee is a separate payment, after the zap; if it fails, the zap stands.",
                    sats(q.amount)
                )
            } else {
                String::new()
            },
            privacy: String::new(),
            total: sats(q.total),
            wallet: q.wallet,
        }
        .with_privacy(&q))
    }

    /// Zaps from the connected wallet; what happened, in words.
    pub async fn zap_from_wallet(
        &self,
        amount: u64,
        comment: String,
        anon: bool,
    ) -> Result<WalletZap, String> {
        let (g, id, who) = self.target().await?;
        let sent = self
            .app
            .zap(&g, &id, amount, &comment, anon)
            .await
            .map_err(|e| e.to_string())?;
        let mut out = if sent.counted {
            format!("Zapped {} sats to {who}.", sats(amount))
        } else {
            format!(
                "Paid {} sats to {who}. Waiting for its receipt to reach the group…",
                sats(amount)
            )
        };
        out += &fee_note(sent.fee);
        if sent.counted {
            let mut s = self.s.lock().await;
            self.zap_totals(&mut s, Some(vec![id])).await;
            self.render(&mut s);
        }
        Ok(WalletZap {
            result: out,
            counted: sent.counted,
            fee: sent.fee,
        })
    }

    /// After a wallet zap not counted at once: waits for the anchor to
    /// carry its receipt in, as after a QR zap; what happened.
    pub async fn wallet_zap_counted(&self, amount: u64, fee: u64) -> String {
        let counted = self.app.zap_wait().await.unwrap_or(false);
        let Ok((_, id, who)) = self.target().await else {
            return String::new();
        };
        let mut s = self.s.lock().await;
        self.zap_totals(&mut s, Some(vec![id])).await;
        self.render(&mut s);
        let out = if counted {
            format!("Zapped {} sats to {who}.", sats(amount))
        } else {
            format!(
                "Paid {} sats to {who}, but no receipt has reached the group in two minutes. The zap went; the total appears if the receipt ever does.",
                sats(amount)
            )
        };
        out + &fee_note(fee)
    }

    /// The zap's invoice, for any wallet to pay by QR code.
    pub async fn zap_invoice(&self, amount: u64, comment: String, anon: bool) -> Result<Invoice, String> {
        let (g, id, _) = self.target().await?;
        let inv = self
            .app
            .zap_invoice(&g, &id, amount, &comment, anon)
            .await
            .map_err(|e| e.to_string())?;
        invoice(&inv)
    }

    /// Waits for the QR zap to be paid and counted; what happened.
    pub async fn zap_wait(&self, amount: u64) -> String {
        let counted = self.app.zap_wait().await.unwrap_or(false);
        let Ok((_, id, who)) = self.target().await else {
            return String::new();
        };
        let mut s = self.s.lock().await;
        self.zap_totals(&mut s, Some(vec![id])).await;
        self.render(&mut s);
        if counted {
            format!("Zapped {} sats to {who}.", sats(amount))
        } else {
            "No receipt yet. If you paid, the total appears when the group's anchor gets it.".into()
        }
    }

    /// The fee's invoice, after a QR zap — if this build charges one.
    pub async fn zap_fee_invoice(&self) -> Result<Option<Invoice>, String> {
        match self.app.zap_fee_invoice().await.map_err(|e| e.to_string())? {
            Some(inv) => invoice(&inv).map(Some),
            None => Ok(None),
        }
    }

    /// Zap totals for `ids` (or every message on screen), read again.
    pub(super) async fn zap_totals(&self, s: &mut State, ids: Option<Vec<String>>) {
        let want = ids.unwrap_or_else(|| s.messages.iter().map(|m| m.id.clone()).collect());
        if want.is_empty() {
            return;
        }
        let Ok(totals) = self.app.zap_totals(&want).await else { return };
        for m in &mut s.messages {
            if want.contains(&m.id) {
                let (sats, count) = totals.get(&m.id).copied().unwrap_or((0, 0));
                m.zap_sats = sats;
                m.zap_count = count;
            }
        }
    }
}

impl QuoteText {
    /// DESIGN.md §11: a zap is public by nature — said before paying.
    fn with_privacy(mut self, q: &ZapQuoteView) -> Self {
        self.privacy = format!(
            "A zap is public by nature: {}'s server sees who you are (unless anonymous), the amount, your comment and which message this is, and your IP address. Its receipt goes to {}.",
            q.address,
            q.relays.join(", ")
        );
        self
    }
}

/// An invoice and its QR code: the `lightning:` URI, upper-cased (the
/// alphanumeric mode, a smaller code), 4 pixels a module, a 4-module quiet
/// zone.
fn invoice(inv: &InvoiceView) -> Result<Invoice, String> {
    const SCALE: usize = 4;
    const QUIET: usize = 4;
    let code = qrcode::QrCode::new(format!("lightning:{}", inv.bolt11.to_uppercase()))
        .map_err(|e| e.to_string())?;
    let n = code.width();
    let side = (n + 2 * QUIET) * SCALE;
    let colors = code.to_colors();
    let mut px = SharedPixelBuffer::<Rgba8Pixel>::new(
        u32::try_from(side).map_err(|e| e.to_string())?,
        u32::try_from(side).map_err(|e| e.to_string())?,
    );
    let white = Rgba8Pixel { r: 255, g: 255, b: 255, a: 255 };
    let black = Rgba8Pixel { r: 0, g: 0, b: 0, a: 255 };
    for (i, p) in px.make_mut_slice().iter_mut().enumerate() {
        let (x, y) = (i % side / SCALE, i / side / SCALE);
        let dark = x >= QUIET
            && y >= QUIET
            && x < n + QUIET
            && y < n + QUIET
            && colors[(y - QUIET) * n + (x - QUIET)] == qrcode::Color::Dark;
        *p = if dark { black } else { white };
    }
    Ok(Invoice {
        bolt11: inv.bolt11.clone(),
        sats: sats(inv.sats),
        qr: px,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sats_have_thousands() {
        assert_eq!(sats(0), "0");
        assert_eq!(sats(21), "21");
        assert_eq!(sats(1000), "1,000");
        assert_eq!(sats(1_234_567), "1,234,567");
    }

    #[test]
    fn a_qr_code_is_square_with_its_quiet_zone() {
        let inv = invoice(&InvoiceView {
            bolt11: "lnbc10n1pjexample".into(),
            qr: String::new(),
            sats: 1,
        })
        .unwrap();
        assert_eq!(inv.qr.width(), inv.qr.height());
        // The corner is quiet zone: white.
        assert_eq!(inv.qr.as_slice()[0].r, 255);
    }
}
