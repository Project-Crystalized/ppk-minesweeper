//! Network chat relay.
//!
//! Velocity mirrors chat between lobby and (this) minesweeper server:
//! every client chat passes the proxy first, so its PlayerChatEvent listener
//! relays a rendered line to every player's current server over the
//! `crystalized:main` plugin channel ("Chat" payload, see velocity's
//! ChatRelay.java).
//!
//! Here we only receive: print relayed lines as system messages, and cancel
//! the vanilla chat broadcast so relay lines are the single source of chat
//! on this server (no duplicates).

use pumpkin_plugin_api::{
    events::{EventData, EventHandler, PlayerChatEvent, PlayerCustomPayloadEvent},
    Server,
};

/// Receives relayed chat from the proxy.
pub struct ChatRelayListener;

impl EventHandler<PlayerCustomPayloadEvent> for ChatRelayListener {
    fn handle(&self, _server: Server, event: EventData<PlayerCustomPayloadEvent>) -> EventData<PlayerCustomPayloadEvent> {
        if event.channel != "crystalized:main" {
            return event;
        }
        // Payload layout is Java DataOutput UTFs: ["Chat", origin, line].
        let mut off = 0usize;
        let tag = read_java_utf(&event.data, &mut off);
        if tag.as_deref() != Some("Chat") {
            return event;
        }
        let _origin = read_java_utf(&event.data, &mut off);
        let Some(line) = read_java_utf(&event.data, &mut off) else {
            return event;
        };
        if line.is_empty() {
            return event;
        }
        event.player.send_system_message(
            pumpkin_plugin_api::text::TextComponent::from_legacy_string(&line),
            false,
        );
        event
    }
}

/// Suppresses this server's vanilla chat broadcast: the proxy relay already
/// shows every message (including ones typed here), in the same format
/// lobby players see. Without this, pumpkin's plain `name: message` line
/// would duplicate the styled relay line.
pub struct ChatSuppressListener;

impl EventHandler<PlayerChatEvent> for ChatSuppressListener {
    fn handle(&self, _server: Server, mut event: EventData<PlayerChatEvent>) -> EventData<PlayerChatEvent> {
        event.cancelled = true;
        event
    }
}

/// Java `DataInput.readUTF`-style string: 2-byte big-endian length, then
/// UTF-8 bytes. Good enough for our ASCII/sane-UTF payload; `NUL` encodings
/// cannot appear in vanilla chat.
fn read_java_utf(data: &[u8], off: &mut usize) -> Option<String> {
    let len = {
        let len_bytes = data.get(*off..*off + 2)?;
        *off += 2;
        ((len_bytes[0] as usize) << 8) | len_bytes[1] as usize
    };
    let bytes = data.get(*off..*off + len)?;
    *off += len;
    String::from_utf8(bytes.to_vec()).ok()
}
