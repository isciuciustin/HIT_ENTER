//! Just enough of a Nostr relay client (NIP-01) for NWC and zap receipts:
//! publish an event and wait for its `OK`, subscribe and read events.
//!
//! `wss://` anywhere; plain `ws://` only to this machine (a fake).

use std::{collections::VecDeque, time::Duration};

use futures_util::{SinkExt, StreamExt};
use nostr::prelude::{ClientMessage, Event, Filter, RelayMessage, SubscriptionId};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{Message, protocol::WebSocketConfig},
};

use crate::{Error, is_loopback, tls};

/// The largest message read from a relay.
const MAX_MESSAGE: usize = 512 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub struct Relay {
    url: String,
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    /// Events for a subscription that arrived while waiting for an `OK`.
    pending: VecDeque<(SubscriptionId, Event)>,
}

impl Relay {
    pub async fn connect(url: &str) -> Result<Self, Error> {
        let err = |why: &str| Error::Relay(url.to_string(), why.to_string());
        let parsed = reqwest::Url::parse(url).map_err(|_| err("not a URL"))?;
        let host = parsed.host_str().unwrap_or("");
        match parsed.scheme() {
            "wss" => {}
            "ws" if is_loopback(host) => {}
            _ => return Err(err("not a wss:// relay")),
        }
        tls();
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE))
            .max_frame_size(Some(MAX_MESSAGE));
        let (ws, _) = tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio_tungstenite::connect_async_with_config(url, Some(config), false),
        )
        .await
        .map_err(|_| err("timed out"))?
        .map_err(|_| err("could not connect"))?;
        Ok(Self {
            url: url.to_string(),
            ws,
            pending: VecDeque::new(),
        })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    fn err(&self, why: impl Into<String>) -> Error {
        Error::Relay(self.url.clone(), why.into())
    }

    async fn send(&mut self, msg: ClientMessage<'_>) -> Result<(), Error> {
        let text = msg.as_json();
        self.ws
            .send(Message::Text(text.into()))
            .await
            .map_err(|_| self.err("connection lost"))
    }

    async fn recv(&mut self) -> Result<RelayMessage<'static>, Error> {
        loop {
            match self.ws.next().await {
                Some(Ok(Message::Text(t))) => {
                    if let Ok(m) = RelayMessage::from_json(t.as_str()) {
                        return Ok(m);
                    }
                }
                Some(Ok(Message::Ping(p))) => {
                    let _ = self.ws.send(Message::Pong(p)).await;
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                    return Err(self.err("connection closed"));
                }
                Some(Ok(_)) => {}
            }
        }
    }

    /// Publishes `event` and waits for the relay's `OK`.
    pub async fn publish(&mut self, event: &Event, timeout: Duration) -> Result<(), Error> {
        self.send(ClientMessage::event(event.clone())).await?;
        let wait = async {
            loop {
                match self.recv().await? {
                    RelayMessage::Ok {
                        event_id,
                        status,
                        message,
                    } if event_id == event.id => {
                        return if status {
                            Ok(())
                        } else {
                            Err(self.err(format!("refused: {message}")))
                        };
                    }
                    RelayMessage::Event {
                        subscription_id,
                        event,
                    } => self
                        .pending
                        .push_back((subscription_id.into_owned(), event.into_owned())),
                    _ => {}
                }
            }
        };
        tokio::time::timeout(timeout, wait)
            .await
            .map_err(|_| self.err("no OK"))?
    }

    /// Opens subscription `id` for `filter`.
    pub async fn subscribe(&mut self, id: &str, filter: Filter) -> Result<(), Error> {
        self.send(ClientMessage::req(SubscriptionId::new(id), vec![filter]))
            .await
    }

    pub async fn close(&mut self, id: &str) {
        let _ = self.send(ClientMessage::close(SubscriptionId::new(id))).await;
    }

    /// The next event on subscription `id`, stored or live. `None` at the
    /// relay's `EOSE` when `stop_at_eose`, or when it closes the
    /// subscription.
    pub async fn next_event(&mut self, id: &str, stop_at_eose: bool) -> Result<Option<Event>, Error> {
        let sub = SubscriptionId::new(id);
        if let Some(i) = self.pending.iter().position(|(s, _)| *s == sub) {
            return Ok(self.pending.remove(i).map(|(_, e)| e));
        }
        loop {
            match self.recv().await? {
                RelayMessage::Event {
                    subscription_id,
                    event,
                } if *subscription_id == sub => return Ok(Some(event.into_owned())),
                RelayMessage::EndOfStoredEvents(s) if *s == sub && stop_at_eose => return Ok(None),
                RelayMessage::Closed {
                    subscription_id,
                    message,
                } if *subscription_id == sub => {
                    return Err(self.err(format!("closed: {message}")));
                }
                _ => {}
            }
        }
    }
}

/// A random subscription id.
pub(crate) fn sub_id() -> String {
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    hex::encode(b)
}
