//! Emoji data for the picker (Unicode 15.0, drawn with the bundled Twemoji font).

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Emoji {
    pub group: &'static str,
    pub emoji: &'static str,
    pub name: &'static str,
}

/// Picker categories in display order, with a representative emoji.
pub const GROUPS: [(&str, &str); 9] = [
    ("smileys", "😀"),
    ("people", "👋"),
    ("nature", "🐻"),
    ("food", "🍔"),
    ("activities", "⚽"),
    ("travel", "🚗"),
    ("objects", "💡"),
    ("symbols", "❤️"),
    ("flags", "🏁"),
];

pub fn all() -> &'static [Emoji] {
    static LIST: OnceLock<Vec<Emoji>> = OnceLock::new();
    LIST.get_or_init(|| {
        include_str!("../assets/emoji.tsv")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .filter_map(|l| {
                let mut parts = l.splitn(3, '\t');
                Some(Emoji { group: parts.next()?, emoji: parts.next()?, name: parts.next()? })
            })
            .collect()
    })
}

/// Emoji whose name contains every word of the query.
pub fn search(query: &str) -> Vec<Emoji> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    all().iter().filter(|e| words.iter().all(|w| e.name.contains(w.as_str()))).copied().collect()
}

pub fn in_group(group: &str) -> impl Iterator<Item = Emoji> + '_ {
    all().iter().filter(move |e| e.group == group).copied()
}

/// Quick reactions offered first (like Discord's frequently used).
pub const QUICK: [&str; 8] = ["👍", "❤️", "😂", "😮", "😢", "🎉", "🔥", "👀"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_loads_and_searches() {
        assert!(all().len() > 1500);
        assert!(GROUPS.iter().all(|(g, _)| in_group(g).next().is_some()));
        let hits = search("thumbs up");
        assert!(hits.iter().any(|e| e.emoji == "👍"), "{hits:?}");
        assert!(search("   ").is_empty());
    }
}
