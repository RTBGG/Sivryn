//! Messages as the client keeps them.
//!
//! The server stores three kinds of encrypted records per channel: messages,
//! edits and reactions (the latter two point at a message). The client keeps
//! the decrypted records ([`Record`]) and folds them into what the UI shows
//! ([`ChatMessage`]).

use sivryn_client_core::protocol::model::{MessageContent, MessageKind};
use sivryn_client_core::protocol::{MessageId, UserId};

/// One decrypted record from the server.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub id: MessageId,
    pub author: UserId,
    /// Unix time in milliseconds.
    pub created_at: i64,
    pub kind: MessageKind,
    pub target: Option<MessageId>,
    /// `None` when this device cannot decrypt it (sent before it joined).
    pub content: Option<MessageContent>,
}

/// A message as shown in the chat, with edits and reactions applied.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    pub id: MessageId,
    pub author: UserId,
    pub created_at: i64,
    pub content: Option<MessageContent>,
    pub edited: bool,
    /// Emoji with the users who reacted, in order of first use.
    pub reactions: Vec<(String, Vec<UserId>)>,
}

impl ChatMessage {
    pub fn body(&self) -> Option<&str> {
        self.content.as_ref().map(|c| c.body.as_str())
    }

    fn from_record(r: &Record) -> Self {
        Self {
            id: r.id,
            author: r.author,
            created_at: r.created_at,
            content: r.content.clone(),
            edited: false,
            reactions: Vec::new(),
        }
    }

    /// Applies an edit or reaction aimed at this message.
    pub fn apply(&mut self, action: &Record) {
        let Some(content) = &action.content else { return };
        match action.kind {
            // Only the author may edit (the server checks too).
            MessageKind::Edit if action.author == self.author => {
                if let Some(current) = &mut self.content {
                    current.body = content.body.clone();
                    current.embeds = content.embeds.clone();
                } else {
                    self.content = Some(content.clone());
                }
                self.edited = true;
            }
            MessageKind::Reaction => {
                let Some(reaction) = &content.reaction else { return };
                let pos = self.reactions.iter().position(|(e, _)| *e == reaction.emoji);
                match (pos, reaction.remove) {
                    (Some(i), true) => {
                        self.reactions[i].1.retain(|u| *u != action.author);
                        if self.reactions[i].1.is_empty() {
                            self.reactions.remove(i);
                        }
                    }
                    (Some(i), false) => {
                        if !self.reactions[i].1.contains(&action.author) {
                            self.reactions[i].1.push(action.author);
                        }
                    }
                    (None, false) => self.reactions.push((reaction.emoji.clone(), vec![action.author])),
                    (None, true) => {}
                }
            }
            _ => {}
        }
    }
}

/// Builds the chat from records (any order): messages sorted by time, each
/// with its edits and reactions applied in the order they were sent.
pub fn fold(records: &[Record]) -> Vec<ChatMessage> {
    let mut messages: Vec<ChatMessage> =
        records.iter().filter(|r| r.kind == MessageKind::Message).map(ChatMessage::from_record).collect();
    messages.sort_by_key(|m| (m.created_at, m.id.0));
    let mut actions: Vec<&Record> = records.iter().filter(|r| r.kind != MessageKind::Message).collect();
    actions.sort_by_key(|r| r.id.0);
    for action in actions {
        if let Some(m) = messages.iter_mut().find(|m| Some(m.id) == action.target) {
            m.apply(action);
        }
    }
    messages
}

/// Adds new records to an already folded chat (from the gateway or a
/// catch-up), skipping duplicates.
pub fn merge(list: &mut Vec<ChatMessage>, records: impl IntoIterator<Item = Record>) {
    for record in records {
        match record.kind {
            MessageKind::Message => match list.iter().position(|m| m.id == record.id) {
                // A second copy can't be decrypted again; keep the readable one.
                Some(i) if list[i].content.is_none() => list[i] = ChatMessage::from_record(&record),
                Some(_) => {}
                None => {
                    list.push(ChatMessage::from_record(&record));
                    list.sort_by_key(|m| (m.created_at, m.id.0));
                }
            },
            _ => {
                if let Some(m) = list.iter_mut().find(|m| Some(m.id) == record.target) {
                    m.apply(&record);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use sivryn_client_core::protocol::model::Reaction;

    use super::*;

    fn msg(author: UserId, at: i64, body: &str) -> Record {
        Record {
            id: MessageId::new(),
            author,
            created_at: at,
            kind: MessageKind::Message,
            target: None,
            content: Some(MessageContent { body: body.into(), ..Default::default() }),
        }
    }

    fn action(author: UserId, target: MessageId, kind: MessageKind, content: MessageContent) -> Record {
        Record { id: MessageId::new(), author, created_at: 0, kind, target: Some(target), content: Some(content) }
    }

    fn react(author: UserId, target: MessageId, emoji: &str, remove: bool) -> Record {
        let content = MessageContent { reaction: Some(Reaction { emoji: emoji.into(), remove }), ..Default::default() };
        action(author, target, MessageKind::Reaction, content)
    }

    #[test]
    fn edits_and_reactions_fold_in_order() {
        let (alice, bob) = (UserId::new(), UserId::new());
        let first = msg(alice, 1, "helo");
        let second = msg(bob, 2, "hi");
        let edit =
            action(alice, first.id, MessageKind::Edit, MessageContent { body: "hello".into(), ..Default::default() });
        // Someone else can't edit Alice's message.
        let forged =
            action(bob, first.id, MessageKind::Edit, MessageContent { body: "pwned".into(), ..Default::default() });
        let records = vec![
            second.clone(),
            react(bob, first.id, "👍", false),
            edit,
            forged,
            react(alice, first.id, "👍", false),
            react(bob, first.id, "🎉", false),
            react(bob, first.id, "👍", true),
            first.clone(),
        ];
        let chat = fold(&records);
        assert_eq!(chat.iter().map(|m| m.id).collect::<Vec<_>>(), [first.id, second.id]);
        assert_eq!(chat[0].body(), Some("hello"));
        assert!(chat[0].edited);
        assert_eq!(chat[0].reactions, vec![("👍".to_string(), vec![alice]), ("🎉".to_string(), vec![bob])]);
    }

    #[test]
    fn merge_skips_duplicates_and_applies_actions() {
        let alice = UserId::new();
        let m = msg(alice, 1, "x");
        let mut chat = fold(std::slice::from_ref(&m));
        let unreadable = Record { content: None, ..m.clone() };
        merge(&mut chat, [unreadable, react(alice, m.id, "❤️", false)]);
        assert_eq!(chat.len(), 1);
        assert_eq!(chat[0].body(), Some("x"));
        assert_eq!(chat[0].reactions.len(), 1);
    }
}
