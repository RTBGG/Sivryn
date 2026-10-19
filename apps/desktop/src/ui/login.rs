use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;

use super::AppState;
use crate::session::Session;

#[component]
pub fn Login() -> Element {
    let mut state = use_context::<AppState>();
    let mut instance = use_signal(|| "https://".to_string());
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut register = use_signal(|| false);
    let mut busy = use_signal(|| false);

    let submit = move |_| {
        if *busy.read() {
            return;
        }
        busy.set(true);
        state.status.set(None);
        spawn(async move {
            // Clone out of the signals: their read guards must not live across `.await`.
            let (instance, username, password) = (instance.cloned(), username.cloned(), password.cloned());
            let result = Session::start(&instance, &username, &password, *register.peek()).await;
            busy.set(false);
            match result {
                Ok(session) => {
                    state.session.set(Some(session));
                    // Outlives this component, which unmounts after login.
                    spawn_forever(state.run_gateway());
                }
                Err(e) => state.status.set(Some(e)),
            }
        });
    };

    let title = if *register.read() { "Create an account" } else { "Welcome back" };
    let action = if *register.read() { "Register" } else { "Log in" };
    let toggle = if *register.read() { "Already have an account? Log in" } else { "New here? Register" };

    rsx! {
        div { class: "login",
            div { class: "card",
                h1 { class: "brand", "Sivryn" }
                p { class: "muted", "{title}" }
                label { "Instance" }
                input {
                    value: "{instance}",
                    oninput: move |e| instance.set(e.value()),
                }
                label { "Username" }
                input {
                    value: "{username}",
                    oninput: move |e| username.set(e.value()),
                }
                label { "Password" }
                input {
                    r#type: "password",
                    value: "{password}",
                    oninput: move |e| password.set(e.value()),
                }
                if let Some(err) = state.status.read().as_ref() {
                    p { class: "error", "{err}" }
                }
                button { class: "primary", onclick: submit,
                    if *busy.read() { "…" } else { "{action}" }
                }
                p { class: "link", onclick: move |_| register.toggle(), "{toggle}" }
            }
        }
    }
}
