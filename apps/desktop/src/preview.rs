//! Links in messages: detection and sender-side previews.
//!
//! Previews are built by the sender (like Signal): its client fetches the
//! page or image, uploads the preview image encrypted and puts the result in
//! the message. Receivers never contact the linked site, and the server never
//! sees the URL.

use std::time::Duration;

use sivryn_client_core::protocol::ChannelId;
use sivryn_client_core::protocol::model::Embed;

use crate::files;
use crate::session::Session;

const FETCH_TIMEOUT: Duration = Duration::from_secs(4);
/// Pages and images larger than this aren't previewed.
const MAX_FETCH_BYTES: usize = 8 * 1024 * 1024;
/// Preview images are scaled down to this edge before uploading.
const PREVIEW_EDGE: u32 = 640;

/// A piece of message text: plain text or a link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment<'a> {
    Text(&'a str),
    Link(&'a str),
}

/// Splits text into plain parts and http(s) links. Trailing punctuation that
/// usually ends a sentence isn't part of the link.
pub fn segments(text: &str) -> Vec<Segment<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = find_link_start(rest) {
        if start > 0 {
            out.push(Segment::Text(&rest[..start]));
        }
        let tail = &rest[start..];
        let mut end = tail.find(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '"').unwrap_or(tail.len());
        while end > 0 && tail[..end].ends_with(['.', ',', ';', ':', '!', '?', ')', ']', '\'']) {
            // Keep a closing parenthesis that has a matching opening one.
            if tail[..end].ends_with(')') && tail[..end].matches('(').count() >= tail[..end].matches(')').count() {
                break;
            }
            end -= 1;
        }
        let url = &tail[..end];
        if url.len() > "https://".len() {
            out.push(Segment::Link(url));
        } else {
            out.push(Segment::Text(url));
        }
        rest = &tail[end..];
        if end == 0 {
            out.push(Segment::Text(&rest[..1]));
            rest = &rest[1..];
        }
    }
    if !rest.is_empty() {
        out.push(Segment::Text(rest));
    }
    out
}

fn find_link_start(text: &str) -> Option<usize> {
    let http = text.find("http://");
    let https = text.find("https://");
    match (http, https) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

pub fn first_link(text: &str) -> Option<&str> {
    segments(text).into_iter().find_map(|s| match s {
        Segment::Link(url) => Some(url),
        Segment::Text(_) => None,
    })
}

fn http() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .user_agent(concat!("Mozilla/5.0 (compatible; SivrynLinkPreview/", env!("CARGO_PKG_VERSION"), ")"))
            .build()
            .expect("HTTP client builds")
    })
}

async fn fetch_limited(url: &str) -> Option<(String, Vec<u8>)> {
    let mut resp = http().get(url).send().await.ok()?.error_for_status().ok()?;
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_FETCH_BYTES {
            return None;
        }
    }
    Some((content_type, body))
}

/// Value of a `<meta property|name="…" content="…">` tag.
fn meta(html: &str, key: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(pos) = lower[from..].find("<meta") {
        let start = from + pos;
        let end = start + lower[start..].find('>')?;
        let tag = &html[start..end];
        let tag_lower = &lower[start..end];
        let names = [
            format!("property=\"{key}\""),
            format!("name=\"{key}\""),
            format!("property='{key}'"),
            format!("name='{key}'"),
        ];
        // Pages often ship empty tags (`og:title=""`); those don't count.
        if names.iter().any(|n| tag_lower.contains(n.as_str()))
            && let Some(content) = attr(tag, "content").map(|c| decode_entities(c.trim())).filter(|c| !c.is_empty())
        {
            return Some(content);
        }
        from = end;
    }
    None
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let pos = lower.find(&format!("{name}="))? + name.len() + 1;
    let quote = tag[pos..].chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &tag[pos + 1..];
    Some(&rest[..rest.find(quote)?])
}

fn title_tag(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let open_end = start + lower[start..].find('>')? + 1;
    let close = open_end + lower[open_end..].find("</title>")?;
    let title = decode_entities(html[open_end..close].trim());
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    (!title.is_empty()).then_some(title)
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">")
}

fn absolute(base: &str, link: &str) -> String {
    if link.starts_with("http://") || link.starts_with("https://") {
        return link.to_owned();
    }
    let origin_end =
        base.find("://").map(|i| i + 3).and_then(|i| base[i..].find('/').map(|j| i + j)).unwrap_or(base.len());
    if let Some(rest) = link.strip_prefix("//") {
        let scheme = base.split("://").next().unwrap_or("https");
        return format!("{scheme}://{rest}");
    }
    if link.starts_with('/') {
        return format!("{}{link}", &base[..origin_end]);
    }
    format!("{}/{link}", base[..origin_end].trim_end_matches('/'))
}

/// Downscales, uploads (encrypted) and describes a preview image.
async fn upload_image(
    s: &Session,
    channel: ChannelId,
    bytes: Vec<u8>,
    name: &str,
) -> Option<sivryn_client_core::protocol::model::Attachment> {
    let img = image::load_from_memory(&bytes).ok()?;
    // Keep GIFs as they are so they stay animated; shrink everything else.
    let is_gif = image::guess_format(&bytes).ok() == Some(image::ImageFormat::Gif);
    let data = if is_gif || (img.width() <= PREVIEW_EDGE && img.height() <= PREVIEW_EDGE) {
        bytes
    } else {
        let small = img.thumbnail(PREVIEW_EDGE, PREVIEW_EDGE);
        let mut out = Vec::new();
        small.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
        out
    };
    let file = files::prepare(name.to_owned(), data);
    files::upload(s, channel, &file).await.ok()
}

/// Builds the preview for a link, or `None` if the page has nothing to show.
pub async fn build(s: &Session, channel: ChannelId, url: &str) -> Option<Embed> {
    let (content_type, body) = fetch_limited(url).await?;
    if content_type.starts_with("image/") {
        let name = url.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("image");
        let image = upload_image(s, channel, body, name).await?;
        return Some(Embed {
            url: url.into(),
            title: None,
            description: None,
            site_name: None,
            image: Some(image),
            image_only: true,
        });
    }
    if !content_type.contains("html") {
        return None;
    }
    let html = String::from_utf8_lossy(&body);
    let title = meta(&html, "og:title").or_else(|| meta(&html, "twitter:title")).or_else(|| title_tag(&html));
    let description = meta(&html, "og:description").or_else(|| meta(&html, "description"));
    let site_name = meta(&html, "og:site_name");
    let image_url = meta(&html, "og:image").or_else(|| meta(&html, "twitter:image")).map(|u| absolute(url, &u));
    if title.is_none() && description.is_none() && image_url.is_none() {
        return None;
    }
    let mut image = None;
    if let Some(image_url) = image_url
        && let Some((ct, bytes)) = fetch_limited(&image_url).await
        && ct.starts_with("image/")
    {
        image = upload_image(s, channel, bytes, "preview").await;
    }
    let clip = |s: String, n: usize| {
        if s.chars().count() > n { format!("{}…", s.chars().take(n).collect::<String>()) } else { s }
    };
    Some(Embed {
        url: url.into(),
        title: title.map(|t| clip(t, 200)),
        description: description.map(|d| clip(d, 350)),
        site_name: site_name.map(|n| clip(n, 80)),
        image,
        image_only: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_links_without_trailing_punctuation() {
        let s = segments("Look: https://example.com/a?b=1, and (see http://x.org/wiki/Foo_(bar)).");
        assert_eq!(
            s,
            vec![
                Segment::Text("Look: "),
                Segment::Link("https://example.com/a?b=1"),
                Segment::Text(", and (see "),
                Segment::Link("http://x.org/wiki/Foo_(bar)"),
                Segment::Text(")."),
            ]
        );
        assert_eq!(segments("no links"), vec![Segment::Text("no links")]);
        assert_eq!(first_link("a https://b.c d"), Some("https://b.c"));
        assert_eq!(segments("https://"), vec![Segment::Text("https://")]);
    }

    #[test]
    fn reads_open_graph_tags() {
        let html = r#"<html><head><title>Fallback</title>
            <meta property="og:title" content="Sivryn &amp; friends">
            <meta name="description" content='Self-hosted chat'>
            <meta property="og:image" content="/img/card.png"></head></html>"#;
        assert_eq!(meta(html, "og:title").as_deref(), Some("Sivryn & friends"));
        assert_eq!(meta(html, "description").as_deref(), Some("Self-hosted chat"));
        assert_eq!(title_tag(html).as_deref(), Some("Fallback"));
        assert_eq!(absolute("https://sivryn.com/blog/post", "/img/card.png"), "https://sivryn.com/img/card.png");
        assert_eq!(absolute("https://sivryn.com/x", "//cdn.example/a.png"), "https://cdn.example/a.png");
    }

    #[test]
    fn empty_tags_fall_back() {
        let html = "<title>
  Rust Programming Language
</title><meta property=\"og:title\" content=\"\" />";
        assert_eq!(meta(html, "og:title"), None);
        assert_eq!(title_tag(html).as_deref(), Some("Rust Programming Language"));
    }
}
