//! Mentions of people and channels in messages.
//!
//! Messages carry them as tokens, `<@user-id>` and `<#channel-id>` (like
//! Discord), so renaming someone or a channel doesn't break old messages;
//! they're shown with the current name. The message box shows `@Name` and
//! `#name` instead and remembers which token each of those stands for.

use sivryn_client_core::protocol::{ChannelId, UserId};

/// A mention token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    User(UserId),
    Channel(ChannelId),
}

impl Token {
    pub fn encode(self) -> String {
        match self {
            Token::User(id) => format!("<@{id}>"),
            Token::Channel(id) => format!("<#{id}>"),
        }
    }
}

/// A token at the start of `text`, with its length in bytes.
pub fn parse_token(text: &str) -> Option<(Token, usize)> {
    let kind = text.get(..2)?;
    if kind != "<@" && kind != "<#" {
        return None;
    }
    let end = text[2..].find('>')? + 2;
    let id = &text[2..end];
    let token = match kind {
        "<@" => Token::User(id.parse().ok()?),
        _ => Token::Channel(id.parse().ok()?),
    };
    Some((token, end + 1))
}

/// Pieces of text and tokens, in order.
pub fn split(text: &str) -> Vec<Result<&str, Token>> {
    let mut out = Vec::new();
    let mut plain_start = 0;
    let mut i = 0;
    while let Some(offset) = text[i..].find('<') {
        let at = i + offset;
        match parse_token(&text[at..]) {
            Some((token, len)) => {
                if at > plain_start {
                    out.push(Ok(&text[plain_start..at]));
                }
                out.push(Err(token));
                i = at + len;
                plain_start = i;
            }
            None => i = at + 1,
        }
    }
    if plain_start < text.len() {
        out.push(Ok(&text[plain_start..]));
    }
    out
}

/// A mention picked in the message box: the text shown there (`@Name`)
/// and the token it stands for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Picked {
    pub label: String,
    pub token: Token,
}

/// The message to send: picked labels still in the draft become tokens
/// (longest first, so "@Ann Lee" wins over "@Ann").
pub fn encode(draft: &str, picked: &[Picked]) -> String {
    let mut picked: Vec<&Picked> = picked.iter().collect();
    picked.sort_by_key(|p| std::cmp::Reverse(p.label.len()));
    let mut out = draft.to_owned();
    for p in picked {
        out = out.replace(&p.label, &p.token.encode());
    }
    out
}

/// A message back in the message box (editing): tokens become labels.
pub fn decode(body: &str, label: impl Fn(Token) -> String) -> (String, Vec<Picked>) {
    let mut draft = String::new();
    let mut picked = Vec::new();
    for piece in split(body) {
        match piece {
            Ok(text) => draft.push_str(text),
            Err(token) => {
                let l = label(token);
                draft.push_str(&l);
                if !picked.iter().any(|p: &Picked| p.label == l) {
                    picked.push(Picked { label: l, token });
                }
            }
        }
    }
    (draft, picked)
}

/// Plain text for notifications and previews: tokens become names.
pub fn plain(body: &str, label: impl Fn(Token) -> String) -> String {
    split(body).into_iter().map(|p| p.map(str::to_owned).unwrap_or_else(&label)).collect()
}

/// A mention being typed at the end of the draft: the trigger (`@` or `#`),
/// what follows it, and the byte index of the trigger. The trigger must
/// start the text or follow a space.
pub fn query(draft: &str) -> Option<(char, &str, usize)> {
    let start = draft.rfind(['@', '#'])?;
    let trigger = draft[start..].chars().next()?;
    let typed = &draft[start + 1..];
    if typed.chars().any(char::is_whitespace) {
        return None;
    }
    if start > 0 && !draft[..start].ends_with(char::is_whitespace) {
        return None;
    }
    Some((trigger, typed, start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_round_trip_through_the_message_box() {
        let (ann, general) = (UserId::new(), ChannelId::new());
        let body = format!("hi <@{ann}>, see <#{general}> <notatoken>");
        let pieces = split(&body);
        assert_eq!(pieces[1], Err(Token::User(ann)));
        assert_eq!(pieces[3], Err(Token::Channel(general)));
        assert_eq!(pieces.last(), Some(&Ok(" <notatoken>")));

        let label = |t: Token| match t {
            Token::User(_) => "@Ann Lee".to_owned(),
            Token::Channel(_) => "#general".to_owned(),
        };
        let (draft, picked) = decode(&body, label);
        assert_eq!(draft, "hi @Ann Lee, see #general <notatoken>");
        assert_eq!(encode(&draft, &picked), body);
        assert_eq!(plain(&body, label), draft);
    }

    #[test]
    fn longer_labels_win_and_queries_need_a_space_before() {
        let (a, b) = (UserId::new(), UserId::new());
        let picked = [
            Picked { label: "@Ann".into(), token: Token::User(a) },
            Picked { label: "@Ann Lee".into(), token: Token::User(b) },
        ];
        assert_eq!(encode("@Ann Lee and @Ann", &picked), format!("<@{b}> and <@{a}>"));

        assert_eq!(query("hello @an"), Some(('@', "an", 6)));
        assert_eq!(query("#gen"), Some(('#', "gen", 0)));
        assert_eq!(query("@"), Some(('@', "", 0)));
        assert_eq!(query("mail@example"), None);
        assert_eq!(query("@ann lee"), None);
    }
}
