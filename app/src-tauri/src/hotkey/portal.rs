//! The GlobalShortcuts portal of the desktop (xdg-desktop-portal), for Wayland sessions. There an app can't grab a
//! key: it asks the compositor to bind a shortcut, the user confirms it in the compositor's own dialog, and the
//! compositor says when the shortcut goes down and up.
//!
//! KDE Plasma, GNOME 48 and later and Hyprland have the portal; Sway and the other wlroots compositors don't. That
//! ends here as a failure, which Settings explains with `dictate --toggle` and `dictate --record`.
//!
//! Two shortcuts are bound in one session: the one that dictates, and the one that starts and stops a recording. Each has an id,
//! which the compositor keeps the binding the user chose under.

use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut, Shortcut};
use futures_util::stream::{self, StreamExt};

use super::{portal_key, PortalKey, PortalShortcut, PORTAL_DICTATE};

/// What the portal tells about the shortcut.
pub trait Events: Send + 'static {
    /// The shortcut is bound. `trigger` is how the desktop describes it ("Ctrl+Alt+Space"), empty when it doesn't.
    fn bound(&self, trigger: String);
    /// The desktop describes the shortcut another way now: the user changed it in the desktop's settings.
    fn changed(&self, trigger: String);
    /// The portal is not there, or said no, or has gone away.
    fn failed(&self, reason: String);
    /// A shortcut went down (`true`) or up.
    fn key(&self, which: PortalKey, down: bool);
}

enum Heard {
    Down(PortalKey),
    Up(PortalKey),
    /// How the desktop describes the shortcut that dictates now.
    Changed(String),
    Closed,
}

/// Binds the `shortcuts`, with the wishes of the app for their triggers, and tells `events` what happens to them for as long as
/// the portal lasts. The session is held all that time: closing it would unbind them.
pub async fn run(events: impl Events, shortcuts: [PortalShortcut; 2]) {
    let reason = match listen(&events, &shortcuts).await {
        Ok(()) => "the desktop closed the shortcut session".to_string(),
        Err(reason) => reason,
    };
    events.failed(reason);
}

fn describe(error: ashpd::Error) -> String {
    error.to_string()
}

/// How the desktop describes the shortcut that dictates, among those it lists. That is the one Settings shows.
fn trigger_of(shortcuts: &[Shortcut]) -> String {
    shortcuts
        .iter()
        .find(|shortcut| shortcut.id() == PORTAL_DICTATE)
        .map(|shortcut| shortcut.trigger_description().to_string())
        .unwrap_or_default()
}

/// Returns when the portal ends the session; an error when it could not be had at all.
async fn listen(events: &impl Events, wanted: &[PortalShortcut]) -> Result<(), String> {
    let portal = GlobalShortcuts::new().await.map_err(describe)?;
    let session = portal.create_session().await.map_err(describe)?;

    // Listening starts before the binding, so that a press right after it is not lost.
    let down = portal
        .receive_activated()
        .await
        .map_err(describe)?
        .filter_map(|activated| {
            std::future::ready(portal_key(activated.shortcut_id()).map(Heard::Down))
        });
    let up = portal
        .receive_deactivated()
        .await
        .map_err(describe)?
        .filter_map(|deactivated| {
            std::future::ready(portal_key(deactivated.shortcut_id()).map(Heard::Up))
        });
    let changed = portal
        .receive_shortcuts_changed()
        .await
        .map_err(describe)?
        // Only a change of the shortcut that dictates is news to Settings: the other one is fixed.
        .filter_map(|changed| {
            let ours = changed
                .shortcuts()
                .iter()
                .any(|shortcut| shortcut.id() == PORTAL_DICTATE);
            std::future::ready(ours.then(|| Heard::Changed(trigger_of(changed.shortcuts()))))
        });
    let closed = session
        .receive_closed()
        .await
        .map_err(describe)?
        .map(|()| Heard::Closed);

    let shortcuts: Vec<NewShortcut> = wanted
        .iter()
        .map(|wish| {
            let mut shortcut = NewShortcut::new(wish.id, wish.description);
            if let Some(trigger) = &wish.preferred_trigger {
                shortcut = shortcut.preferred_trigger(trigger.as_str());
            }
            shortcut
        })
        .collect();
    // The compositor may ask the user to confirm: this waits as long as the user takes.
    let bound = portal
        .bind_shortcuts(&session, &shortcuts, None)
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
            Heard::Down(which) => events.key(which, true),
            Heard::Up(which) => events.key(which, false),
            Heard::Changed(trigger) => events.changed(trigger),
            Heard::Closed => break,
        }
    }
    // Best effort: the session is closing anyway.
    let _ = session.close().await;
    Ok(())
}
