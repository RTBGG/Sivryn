use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::*;

use super::AppState;
use crate::session::Session;
use crate::settings::Settings;

#[component]
pub fn Login() -> Element {
    let mut state = use_context::<AppState>();
    // Prefill the last successful login so returning users only type the password.
    let remembered = use_hook(Settings::load);
    let returning = remembered.last_instance.is_some();
    let mut instance = use_signal(|| remembered.last_instance.clone().unwrap_or_else(|| "https://".to_string()));
    let mut username = use_signal(|| remembered.last_username.clone().unwrap_or_default());
    let password = use_signal(String::new);
    let mut invite_code = use_signal(String::new);
    let mut register = use_signal(|| false);
    let mut busy = use_signal(|| false);

    let mut submit = move || {
        if *busy.read() {
            return;
        }
        busy.set(true);
        state.status.set(None);
        spawn(async move {
            // Clone out of the signals: their read guards must not live across `.await`.
            let (instance, username, password) = (instance.cloned(), username.cloned(), password.cloned());
            let code = invite_code.cloned();
            let code = Some(code.trim()).filter(|c| !c.is_empty());
            let result = Session::start(&instance, &username, &password, *register.peek(), code).await;
            busy.set(false);
            match result {
                Ok(session) => {
                    Settings {
                        last_instance: Some(session.api.base_url().to_owned()),
                        last_username: Some(session.user.username.clone()),
                    }
                    .save();
                    state.session.set(Some(session));
                    // Outlives this component, which unmounts after login.
                    spawn_forever(state.run_gateway());
                }
                Err(e) => state.status.set(Some(e)),
            }
        });
    };

    let registering = *register.read();
    let title = if registering { "Create an account" } else { "Welcome back" };
    let action = if registering { "Register" } else { "Log in" };
    let toggle = if registering { "Already have an account? Log in" } else { "New here? Register" };

    rsx! {
        div { class: "login",
            div { class: "card",
                h1 { class: "brand", "Sivryn" }
                p { class: "muted", "{title}" }
                label { "Instance" }
                input {
                    autofocus: !returning,
                    value: "{instance}",
                    oninput: move |e| instance.set(e.value()),
                }
                label { "Username" }
                input {
                    value: "{username}",
                    oninput: move |e| username.set(e.value()),
                }
                label { "Password" }
                PasswordInput {
                    value: password,
                    autofocus: returning,
                    on_enter: move |_| submit(),
                }
                if registering {
                    label { "Invite code" }
                    input {
                        value: "{invite_code}",
                        placeholder: "Only needed if the instance is invite-only",
                        oninput: move |e| invite_code.set(e.value()),
                    }
                }
                if let Some(err) = state.status.read().as_ref() {
                    p { class: "error", "{err}" }
                }
                button { class: "primary", onclick: move |_| submit(),
                    if *busy.read() { "…" } else { "{action}" }
                }
                p { class: "link", onclick: move |_| register.toggle(), "{toggle}" }
            }
        }
    }
}

/// Masked password field with an eye button to reveal the text.
#[component]
fn PasswordInput(value: Signal<String>, autofocus: bool, on_enter: EventHandler<()>) -> Element {
    let mut value = value;
    let mut visible = use_signal(|| false);
    let shown = *visible.read();

    rsx! {
        div { class: "password-field",
            input {
                r#type: if shown { "text" } else { "password" },
                autofocus,
                value: "{value}",
                oninput: move |e| value.set(e.value()),
                onkeydown: move |e| {
                    if e.key() == Key::Enter {
                        on_enter.call(());
                    }
                },
            }
            button {
                class: "eye",
                title: if shown { "Hide password" } else { "Show password" },
                onclick: move |_| visible.toggle(),
                EyeIcon { crossed: shown }
            }
        }
    }
}

#[component]
fn EyeIcon(crossed: bool) -> Element {
    rsx! {
        svg {
            width: "18",
            height: "18",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "#8b8d98",
            stroke_width: "2",
            stroke_linecap: "round",
            stroke_linejoin: "round",
            path { d: "M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z" }
            circle { cx: "12", cy: "12", r: "3" }
            if crossed {
                line { x1: "3", y1: "3", x2: "21", y2: "21" }
            }
        }
    }
}
