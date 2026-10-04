//! the core's log, handed to the page. the core logs through `tracing` like
//! every other embedder's, so what a page shows is word for word what the
//! command line prints: this only carries each line across.

use std::cell::RefCell;
use std::fmt::Write;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Metadata, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;
use wasm_bindgen::JsValue;

// the core's own lines, and not veilid's, which are a developer's.
const OWN_TARGET: &str = "mudita";
const MESSAGE_FIELD: &str = "message";

thread_local! {
    // the page's function for a line: (level, target, message).
    static HANDLER: RefCell<Option<js_sys::Function>> = const { RefCell::new(None) };
}

/// have every line from here on handed to `handler`.
pub fn set_handler(handler: js_sys::Function) {
    HANDLER.with(|held| *held.borrow_mut() = Some(handler));
}

/// whether a line is one the user is shown: the core's own, at info and up.
fn shown(target: &str, level: &Level) -> bool {
    let own = target == OWN_TARGET || target.starts_with("mudita::");
    own && *level <= Level::INFO
}

#[derive(Default)]
struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == MESSAGE_FIELD {
            let _ = write!(self.0, "{value:?}");
        }
    }
}

struct PageLog;

impl<S: Subscriber> Layer<S> for PageLog {
    fn enabled(&self, metadata: &Metadata<'_>, _: Context<'_, S>) -> bool {
        shown(metadata.target(), metadata.level())
    }

    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let mut message = Message::default();
        event.record(&mut message);
        let (level, target) = (event.metadata().level(), event.metadata().target());
        HANDLER.with(|held| {
            if let Some(handler) = &*held.borrow() {
                let said = [level.as_str(), target, &message.0].map(JsValue::from_str);
                // a handler that throws has only lost itself a line.
                let _ = handler.call3(&JsValue::NULL, &said[0], &said[1], &said[2]);
            }
        });
    }
}

/// install the one subscriber of the page. safe to call again.
pub fn install() {
    let _ = tracing_subscriber::registry().with(PageLog).try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_cores_own_lines_at_info_and_up_are_shown() {
        assert!(shown("mudita::peer", &Level::INFO));
        assert!(shown("mudita", &Level::ERROR));
        assert!(shown("mudita::fetch", &Level::WARN));
        assert!(!shown("mudita::peer", &Level::DEBUG));
        assert!(!shown("net", &Level::ERROR));
        assert!(!shown("muditax", &Level::INFO));
    }
}
