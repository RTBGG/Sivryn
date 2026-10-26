use dioxus_native::prelude::*;

use super::AppState;
use super::icons::{Icon, Logo, Svg};
use crate::i18n::t;
use crate::session::Session;

#[component]
pub fn Login() -> Element {
    let state = use_context::<AppState>();
    // Prefill the last login so returning users only type the password.
    let remembered = state.settings.peek().clone();
    let returning = remembered.last_instance.is_some();
    let mut instance = use_signal(|| remembered.last_instance.clone().unwrap_or_else(|| "https://".to_string()));
    let mut username = use_signal(|| remembered.last_username.clone().unwrap_or_default());
    let password = use_signal(String::new);
    let mut invite_code = use_signal(String::new);
    let mut register = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    // An invite link fills in its instance and code. Signing up is the likely
    // next step, unless this is the instance signed in to last.
    use_effect(move || {
        let Some(pending) = state.invite.read().clone() else { return };
        let known = state.settings.peek().last_instance.as_deref().is_some_and(|i| pending.invite.is_for(i));
        instance.set(pending.invite.instance.clone());
        invite_code.set(pending.invite.code.clone());
        if !known {
            register.set(true);
        }
    });

    let mut submit = move || {
        if *busy.read() {
            return;
        }
        busy.set(true);
        error.set(None);
        // Lives as long as this screen; on success the screen unmounts only
        // after `signed_in` handed the session to the root scope.
        spawn(async move {
            // Clone out of the signals: their read guards must not live across `.await`.
            let (instance, username, password) = (instance.cloned(), username.cloned(), password.cloned());
            let code = invite_code.cloned();
            let code = Some(crate::links::code_from(&code)).filter(|c| !c.is_empty());
            let registering = *register.peek();
            let result = Session::login(instance.trim(), username.trim(), &password, registering, code).await;
            busy.set(false);
            match result {
                Ok(session) => {
                    // Signing up with the link's code may have used it up.
                    let mut invite = state.invite;
                    let used =
                        invite.peek().as_ref().is_some_and(|p| registering && Some(p.invite.code.as_str()) == code);
                    if used && let Some(p) = invite.write().as_mut() {
                        p.signed_up = true;
                    }
                    state.signed_in(session)
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };

    let registering = *register.read();
    let (title, subtitle, action, toggle) = if registering {
        (t("login.register_title"), t("login.register_subtitle"), t("login.register"), t("login.have_account"))
    } else {
        (t("login.title"), t("login.subtitle"), t("login.login"), t("login.no_account"))
    };

    rsx! {
        div { class: "login",
            div { class: "login-card",
                div { class: "login-brand",
                    Logo { size: 44 }
                    span { class: "wordmark", "Sivryn" }
                }
                h1 { "{title}" }
                p { class: "muted", "{subtitle}" }
                div { class: "field",
                    label { class: "field-label", {t("login.instance")} }
                    input {
                        class: "input",
                        autofocus: !returning,
                        value: "{instance}",
                        placeholder: "https://chat.example.com",
                        oninput: move |e| instance.set(e.value()),
                    }
                }
                div { class: "field",
                    label { class: "field-label", {t("login.username")} }
                    input { class: "input", value: "{username}", oninput: move |e| username.set(e.value()) }
                }
                div { class: "field",
                    label { class: "field-label", {t("login.password")} }
                    PasswordInput { value: password, autofocus: returning, on_enter: move |_| submit() }
                }
                if registering {
                    div { class: "field",
                        label { class: "field-label", {t("login.invite")} }
                        input {
                            class: "input",
                            value: "{invite_code}",
                            placeholder: t("login.invite_hint"),
                            oninput: move |e| invite_code.set(e.value()),
                        }
                    }
                }
                if let Some(err) = error.read().as_ref() {
                    div { class: "form-error", "{err}" }
                }
                button { class: if *busy.read() { "button primary block busy" } else { "button primary block" }, onclick: move |_| submit(),
                    if *busy.read() { {t("common.wait")} } else { "{action}" }
                }
                p { class: "login-switch", onclick: move |_| register.toggle(), "{toggle}" }
            }
            p { class: "login-footer", {t("login.footer")} }
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
        div { class: "input password-field",
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
            div {
                class: "eye",
                title: if shown { t("login.hide_password") } else { t("login.show_password") },
                onclick: move |_| visible.toggle(),
                if shown {
                    Svg { icon: Icon::EyeOff, size: 18 }
                } else {
                    Svg { icon: Icon::Eye, size: 18 }
                }
            }
        }
    }
}
