//! The native window's [`Sink`]: the node's events, queued for
//! [`crate::chat::Chat`]. A sink must return at once (it is called from the
//! node's tasks), so it only queues; one task applies them in order.

use std::sync::Arc;

use hit_app_core::events::{Sink, UiEvent};
use tokio::sync::mpsc::UnboundedSender;

pub struct Native(UnboundedSender<UiEvent>);

impl Native {
    pub fn sink(to: UnboundedSender<UiEvent>) -> Arc<dyn Sink> {
        Arc::new(Self(to))
    }
}

impl Sink for Native {
    fn emit(&self, event: UiEvent) {
        let _ = self.0.send(event);
    }
}
