//! The GlobalShortcuts portal of the desktop (xdg-desktop-portal), for Wayland sessions. There an app can't grab a
//! key: it asks the compositor to bind a shortcut, the user confirms it in the compositor's own dialog, and the
//! compositor says when the shortcut goes down and up.
//!
//! KDE Plasma, GNOME 48 and later and Hyprland have the portal; Sway and the other wlroots compositors don't. That
//! ends here as a failure, which Settings explains with `dictate --toggle`.

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut, Shortcut};
use futures_util::stream::{self, StreamExt};

/// The id of the shortcut. The compositor keeps the binding the user chose under it.
const SHORTCUT_ID: &str = "dictate";
/// What the compositor's dialog says the shortcut is for.
const DESCRIPTION: &str = "Hold to dictate";

/// What the portal tells about the shortcut.
pub trait Events: Send + 'static {
    /// The shortcut is bound. `trigger` is how the desktop describes it ("Ctrl+Alt+Space"), empty when it doesn't.
    fn bound(&self, trigger: String);
    /// The desktop describes the shortcut another way now: the user changed it in the desktop's settings.
    fn changed(&self, trigger: String);
    /// The portal is not there, or said no, or has gone away.
    fn failed(&self, reason: String);
    /// The shortcut went down (`true`) or up.
    fn key(&self, down: bool);
}

enum Heard {
    Down,
    Up,
    Changed(String),
    Closed,
}

/// Binds the shortcut, with `preferred_trigger` as the wish of the app, and tells `events` what happens to it for as
/// long as the portal lasts. The session is held all that time: closing it would unbind the shortcut.
pub async fn run(events: impl Events, preferred_trigger: Option<String>) {
    let reason = match listen(&events, preferred_trigger.as_deref()).await {
        Ok(()) => "the desktop closed the shortcut session".to_string(),
        Err(reason) => reason,
    };
    events.failed(reason);
}

fn describe(error: ashpd::Error) -> String {
    error.to_string()
}

/// How the desktop describes our shortcut, among those it lists.
fn trigger_of(shortcuts: &[Shortcut]) -> String {
    shortcuts
        .iter()
        .find(|shortcut| shortcut.id() == SHORTCUT_ID)
        .map(|shortcut| shortcut.trigger_description().to_string())
        .unwrap_or_default()
}

/// Returns when the portal ends the session; an error when it could not be had at all.
async fn listen(events: &impl Events, preferred_trigger: Option<&str>) -> Result<(), String> {
    let portal = GlobalShortcuts::new().await.map_err(describe)?;
    let session = portal.create_session().await.map_err(describe)?;

    // Listening starts before the binding, so that a press right after it is not lost.
    let down = portal
        .receive_activated()
        .await
        .map_err(describe)?
        .filter_map(|activated| {
            std::future::ready((activated.shortcut_id() == SHORTCUT_ID).then_some(Heard::Down))
        });
    let up = portal
        .receive_deactivated()
        .await
        .map_err(describe)?
        .filter_map(|deactivated| {
            std::future::ready((deactivated.shortcut_id() == SHORTCUT_ID).then_some(Heard::Up))
        });
    let changed = portal
        .receive_shortcuts_changed()
        .await
        .map_err(describe)?
        .map(|changed| Heard::Changed(trigger_of(changed.shortcuts())));
    let closed = session
        .receive_closed()
        .await
        .map_err(describe)?
        .map(|()| Heard::Closed);

    let mut shortcut = NewShortcut::new(SHORTCUT_ID, DESCRIPTION);
    if let Some(trigger) = preferred_trigger {
        shortcut = shortcut.preferred_trigger(trigger);
    }
    // The compositor may ask the user to confirm: this waits as long as the user takes.
    let bound = portal
        .bind_shortcuts(&session, &[shortcut], None)
        .await
        .map_err(describe)?
        .response()
        .map_err(describe)?;
    events.bound(trigger_of(bound.shortcuts()));

    let mut heard = Box::pin(stream::select(
        stream::select(down, up),
        stream::select(changed, closed),
    ));
    while let Some(message) = heard.next().await {
        match message {
            Heard::Down => events.key(true),
            Heard::Up => events.key(false),
            Heard::Changed(trigger) => events.changed(trigger),
            Heard::Closed => break,
        }
    }
    // Best effort: the session is closing anyway.
    let _ = session.close().await;
    Ok(())
}
