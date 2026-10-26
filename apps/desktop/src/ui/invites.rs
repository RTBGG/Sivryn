//! Invite links (`sivryn://join/…`, see `links.rs`): signed in to the link's
//! instance, Sivryn joins the server right away; signed out, the login screen
//! is filled in with the instance and the code; signed in elsewhere, a dialog
//! offers to sign out and continue there.

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;

use super::icons::Icon;
use super::widgets::IconButton;
use super::{AppState, Modal};
use crate::i18n::t;
use crate::links::{self, Invite};

/// An invite in hand, waiting for a session on its instance.
#[derive(Clone, Debug, PartialEq)]
pub struct PendingInvite {
    pub invite: Invite,
    /// Its code was used to sign up, which may have used it up.
    pub signed_up: bool,
}

/// Picks up an invite link that started Sivryn or reached it since.
pub fn arrived(mut state: AppState) {
    let Some(invite) = links::take() else { return };
    state.invite.set(Some(PendingInvite { invite, signed_up: false }));
    process(state);
}

/// Acts on the pending invite once there is a session (also called after signing in).
pub fn process(mut state: AppState) {
    let Some(pending) = state.invite.peek().clone() else { return };
    // Signed out (or a saved session still being restored, then `signed_in`
    // comes back here): the login screen shows the invite.
    let Some(s) = state.session.peek().clone() else { return };
    if !pending.invite.is_for(s.api.base_url()) {
        state.modal.set(Some(Modal::ForeignInvite));
        return;
    }
    state.invite.set(None);
    spawn_forever(async move {
        match s.api.accept_invite(&pending.invite.code).await {
            Ok(g) => super::shell::open_joined_guild(state, g),
            // Signing up used up an instance invite, which joins no server.
            Err(_) if pending.signed_up => {}
            Err(e) => state.toast(format!("{}: {e}", t("invite.link_failed")), true),
        }
    });
}

/// The invite's instance isn't the one signed in to.
#[component]
pub fn ForeignInviteDialog() -> Element {
    let mut state = use_context::<AppState>();
    let Some(pending) = state.invite.read().clone() else { return rsx! {} };
    let host = |url: &str| url.split_once("://").map_or(url, |(_, rest)| rest).trim_end_matches('/').to_owned();
    let theirs = host(&pending.invite.instance);
    let ours = state.session.read().as_ref().map(|s| host(s.api.base_url())).unwrap_or_default();
    let mut cancel = move || {
        state.invite.set(None);
        state.modal.set(None);
    };
    // The invite stays pending: the login screen picks it up.
    let switch = move |_| {
        spawn_forever(state.sign_out(true));
    };
    rsx! {
        div { class: "overlay", onclick: move |_| cancel(),
            div { class: "dialog", onclick: move |e| e.stop_propagation(),
                div { class: "dialog-header",
                    h2 { {t("invite.foreign_title")} }
                    IconButton { icon: Icon::X, size: 18, onclick: move |_| cancel() }
                }
                p { class: "muted", "{t(\"invite.foreign_text_1\")} {theirs}{t(\"invite.foreign_text_2\")} {ours}{t(\"invite.foreign_text_3\")}" }
                div { class: "dialog-actions",
                    button { class: "button ghost", onclick: move |_| cancel(), {t("common.cancel")} }
                    button { class: "button primary", autofocus: "true", onclick: switch, {t("invite.foreign_action")} }
                }
            }
        }
    }
}
