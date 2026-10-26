//! The instance's web pages: what Sivryn is with a download of the desktop client (`/`), and
//! invite links (`/{code}`, `/invite/{code}`) that open the client at the invite.
//!
//! The client registers the `sivryn://` scheme; `sivryn://join/{host}/{code}` makes it join (or
//! sign up with) the invite on `https://{host}`.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use sivryn_protocol::api::InvitePreview;

use crate::AppState;

const LOGO: &[u8] = include_bytes!("../../../apps/desktop/assets/icons/icon-256.png");

/// How long the newest release is remembered before GitHub is asked again.
const RELEASE_TTL: Duration = Duration::from_secs(600);

pub async fn home(State(state): State<AppState>, headers: HeaderMap) -> Html<String> {
    let lang = Lang::of(&headers);
    let download = download(&state).await;
    let body = format!(
        "<h1>{name}</h1><p class=\"muted\">{tagline}</p>{download}",
        name = escape(&state.config.instance_name),
        tagline = lang.pick(
            "Selbst gehosteter Chat mit Ende-zu-Ende-Verschlüsselung, Sprach- und Videoanrufen und Bildschirmübertragung in hoher Qualität.",
            "Self-hosted chat with end-to-end encryption, voice and video calls and high-quality screen sharing.",
        ),
        download = download_block(lang, download.as_ref(), true),
    );
    Html(page(lang, &state.config.instance_name, &body, None))
}

pub async fn invite(State(state): State<AppState>, Path(code): Path<String>, headers: HeaderMap) -> Response {
    let lang = Lang::of(&headers);
    // Every other one-segment path lands here too: only plausible codes are looked up.
    let valid =
        (4..=64).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    let preview = match valid {
        true => crate::routes::preview(&state, &code).await.ok().flatten(),
        false => None,
    };
    let Some(preview) = preview else {
        let body = format!(
            "<h1>{}</h1><p class=\"muted\">{}</p>",
            lang.pick("Einladung ungültig", "Invalid invite"),
            lang.pick(
                "Diese Einladung gibt es nicht, oder sie ist abgelaufen.",
                "This invite doesn't exist or has expired.",
            ),
        );
        return (StatusCode::NOT_FOUND, Html(page(lang, &state.config.instance_name, &body, None))).into_response();
    };

    let deep_link = host(&headers).map(|host| format!("sivryn://join/{host}/{code}"));
    let download = download(&state).await;
    let (title, details) = invite_text(lang, &preview);
    let open = match &deep_link {
        Some(link) => format!(
            "<a class=\"button primary\" href=\"{link}\">{}</a>",
            lang.pick("In Sivryn öffnen", "Open in Sivryn"),
            link = escape(link),
        ),
        None => String::new(),
    };
    let body = format!(
        "<p class=\"muted\">{invited}</p><h1>{title}</h1><p class=\"muted\">{details}</p>\
         <div class=\"actions\">{open}</div>\
         <p class=\"code\">{code_label} <span>{code}</span></p>\
         <hr><p class=\"muted small\">{missing}</p>{download}",
        invited = lang.pick("Du wurdest eingeladen zu", "You've been invited to"),
        title = escape(&title),
        details = escape(&details),
        code_label = lang.pick("Einladungscode:", "Invite code:"),
        code = escape(&code),
        missing = lang.pick("Noch kein Sivryn?", "Don't have Sivryn yet?"),
        download = download_block(lang, download.as_ref(), false),
    );
    Html(page(lang, &title, &body, deep_link.as_deref())).into_response()
}

pub async fn logo() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "image/png"), (header::CACHE_CONTROL, "public, max-age=86400")], LOGO)
}

fn invite_text(lang: Lang, preview: &InvitePreview) -> (String, String) {
    match &preview.guild {
        Some(guild) => {
            let members = match (lang, guild.member_count) {
                (Lang::De, 1) => "1 Mitglied".to_owned(),
                (Lang::De, n) => format!("{n} Mitglieder"),
                (Lang::En, 1) => "1 member".to_owned(),
                (Lang::En, n) => format!("{n} members"),
            };
            (guild.name.clone(), format!("{members} · {}", preview.instance_name))
        }
        None => (
            preview.instance_name.clone(),
            lang.pick(
                "Mit dieser Einladung kannst du dir ein Konto anlegen.",
                "This invite lets you create an account.",
            )
            .to_owned(),
        ),
    }
}

/// The download button; `primary` where downloading is the main thing to do.
fn download_block(lang: Lang, download: Option<&Download>, primary: bool) -> String {
    match download {
        Some(d) => format!(
            "<div class=\"actions\"><a class=\"button{primary}\" href=\"{url}\">{label}</a></div><p class=\"muted small\">{version} · Windows 10/11</p>",
            url = escape(&d.url),
            label = lang.pick("Sivryn für Windows herunterladen", "Download Sivryn for Windows"),
            version = escape(&d.version),
            primary = if primary { " primary" } else { "" },
        ),
        None => String::new(),
    }
}

/// The host the page was requested on, as the client should reach the instance.
fn host(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|h| h.to_str().ok())?
        .split(',')
        .next()?
        .trim();
    (!host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b)))
        .then(|| host.to_owned())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    De,
    En,
}

impl Lang {
    fn of(headers: &HeaderMap) -> Self {
        let accept = headers.get(header::ACCEPT_LANGUAGE).and_then(|h| h.to_str().ok()).unwrap_or("");
        if accept.trim_start().to_ascii_lowercase().starts_with("de") { Lang::De } else { Lang::En }
    }

    fn pick(self, de: &'static str, en: &'static str) -> &'static str {
        if self == Lang::De { de } else { en }
    }
}

fn page(lang: Lang, title: &str, body: &str, deep_link: Option<&str>) -> String {
    // Opening the client right away; browsers ask once whether to allow it.
    let open = deep_link
        .map(|link| format!("<script>window.location.href = {};</script>", js_string(link)))
        .unwrap_or_default();
    format!(
        r#"<!doctype html>
<html lang="{lang}">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title} – Sivryn</title>
<meta property="og:title" content="{title}">
<meta property="og:site_name" content="Sivryn">
<meta property="og:image" content="/logo.png">
<link rel="icon" href="/logo.png">
<style>
:root {{ color-scheme: dark; }}
* {{ box-sizing: border-box; }}
body {{ margin: 0; min-height: 100vh; display: flex; align-items: center; justify-content: center; padding: 24px 16px;
  background: #0e0f13; color: #ebecf0; font: 16px/1.5 Inter, "Segoe UI", system-ui, sans-serif; }}
.card {{ width: 100%; max-width: 440px; padding: 32px; border-radius: 16px; background: #15161c; text-align: center;
  box-shadow: 0 12px 40px #00000066; }}
.brand {{ display: flex; align-items: center; justify-content: center; gap: 10px; margin-bottom: 20px; font-weight: 800; font-size: 20px; }}
.brand img {{ width: 36px; height: 36px; }}
h1 {{ margin: 4px 0 6px; font-size: 24px; line-height: 1.25; overflow-wrap: anywhere; }}
p {{ margin: 0 0 12px; }}
.muted {{ color: #a9adba; }}
.small {{ font-size: 14px; color: #6f7483; }}
.actions {{ display: flex; flex-direction: column; gap: 10px; margin: 20px 0 12px; }}
.button {{ display: block; padding: 11px 16px; border-radius: 10px; background: #1b1d24; color: #ebecf0; font-weight: 700; text-decoration: none; }}
.button:hover {{ background: #24262f; }}
.button.primary {{ background: #7c6cff; color: #ffffff; }}
.button.primary:hover {{ background: #6c5cf0; }}
.code {{ font-size: 14px; color: #6f7483; }}
.code span {{ color: #ebecf0; font-family: ui-monospace, Consolas, monospace; user-select: all; }}
hr {{ margin: 20px 0; border: 0; border-top: 1px solid #262833; }}
</style>
</head>
<body>
<main class="card">
<div class="brand"><img src="/logo.png" alt=""><span>Sivryn</span></div>
{body}
</main>
{open}
</body>
</html>"#,
        lang = if lang == Lang::De { "de" } else { "en" },
        title = escape(title),
    )
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// A JavaScript string literal that is also safe inside `<script>`.
fn js_string(text: &str) -> String {
    let json = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
    json.replace('<', "\\u003c").replace('>', "\\u003e")
}

/// The newest desktop installer on GitHub.
#[derive(Clone)]
struct Download {
    url: String,
    version: String,
}

static RELEASE: Mutex<Option<(Instant, Option<Download>)>> = Mutex::new(None);

/// The newest release of the configured repository (pre-releases count), cached for
/// [`RELEASE_TTL`]. Falls back to the releases page when GitHub can't be reached.
async fn download(state: &AppState) -> Option<Download> {
    let repo = state.config.download_repo.clone()?;
    let cached = RELEASE.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some((at, download)) = &cached
        && at.elapsed() < RELEASE_TTL
    {
        return download.clone();
    }
    let fresh = match newest_installer(&repo).await {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(error = %e, "couldn't look up the newest release");
            // Keep what was known; without anything, link the releases page.
            cached.and_then(|(_, d)| d).or_else(|| {
                Some(Download { url: format!("https://github.com/{repo}/releases"), version: "GitHub".into() })
            })
        }
    };
    *RELEASE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), fresh.clone()));
    fresh
}

async fn newest_installer(repo: &str) -> anyhow::Result<Option<Download>> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
        draft: bool,
        assets: Vec<Asset>,
    }
    #[derive(Deserialize)]
    struct Asset {
        name: String,
        browser_download_url: String,
    }

    let releases: Vec<Release> = reqwest::Client::new()
        .get(format!("https://api.github.com/repos/{repo}/releases?per_page=10"))
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", concat!("sivryn-server/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(releases.into_iter().filter(|r| !r.draft).find_map(|r| {
        let asset = r.assets.into_iter().find(|a| a.name.ends_with("-Setup.exe"))?;
        Some(Download { url: asset.browser_download_url, version: r.tag_name })
    }))
}
