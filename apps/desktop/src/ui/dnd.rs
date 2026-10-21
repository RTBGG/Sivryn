//! Dragging in the channel list: channels and categories to reorder them,
//! people in voice channels to move them to another voice channel.
//!
//! Blitz has no HTML drag and drop and reports only window coordinates, so
//! dragging is done by hand: a left press starts a pending drag, moving a few
//! pixels makes it active, and while it is active every row shows invisible
//! drop zones (its top and bottom half) that set the drop target. The window
//! root follows the mouse for the ghost and finishes the drag on release.

use dioxus_native::prelude::dioxus_core::spawn_forever;
use dioxus_native::prelude::dioxus_elements::input_data::MouseButton;
use dioxus_native::prelude::*;
use sivryn_client_core::protocol::api::{ChannelPlacement, LayoutRequest};
use sivryn_client_core::protocol::model::{Channel, ChannelKind};
use sivryn_client_core::protocol::{CategoryId, ChannelId, GuildId, UserId};

use super::AppState;
use super::icons::{Icon, Svg};
use super::widgets::Avatar;

/// What is being dragged.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DragItem {
    Channel(ChannelId),
    Category(CategoryId),
    /// Someone in a voice channel.
    Member {
        user: UserId,
        from: ChannelId,
    },
}

/// Where it would land.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DropTarget {
    BeforeChannel(ChannelId),
    AfterChannel(ChannelId),
    /// At the top of a category.
    IntoCategory(CategoryId),
    BeforeCategory(CategoryId),
    AfterCategory(CategoryId),
    /// Into a voice channel (people).
    Voice(ChannelId),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Drag {
    pub guild: GuildId,
    pub item: DragItem,
    start: (f64, f64),
    pub pos: (f64, f64),
    /// Moved far enough to count as a drag rather than a click.
    pub active: bool,
    pub target: Option<DropTarget>,
}

/// Pixels the mouse must move before a press becomes a drag.
const THRESHOLD: f64 = 5.0;

thread_local! {
    /// When the last drag ended: the click that follows the release must
    /// not open or join what was dragged.
    static ENDED: std::cell::Cell<Option<std::time::Instant>> = const { std::cell::Cell::new(None) };
}

/// Whether a drag ended just now (the following click should be ignored).
pub fn just_dragged() -> bool {
    ENDED.with(|c| c.get()).is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(250))
}

/// Starts a (pending) drag on a left press.
pub fn begin(mut state: AppState, guild: GuildId, item: DragItem, e: &MouseEvent) {
    if e.trigger_button() != Some(MouseButton::Primary) {
        return;
    }
    let p = e.client_coordinates();
    state.drag.set(Some(Drag { guild, item, start: (p.x, p.y), pos: (p.x, p.y), active: false, target: None }));
}

/// Mouse moved anywhere in the window.
pub fn on_move(mut state: AppState, e: &MouseEvent) {
    if state.drag.peek().is_none() {
        return;
    }
    // A release outside the window never reached us.
    if !e.held_buttons().contains(MouseButton::Primary) {
        finish(state);
        return;
    }
    let p = e.client_coordinates();
    let mut drag = state.drag.write();
    let Some(d) = drag.as_mut() else { return };
    d.pos = (p.x, p.y);
    if !d.active && ((p.x - d.start.0).powi(2) + (p.y - d.start.1).powi(2)).sqrt() > THRESHOLD {
        d.active = true;
    }
}

/// The mouse is over something that isn't a drop zone.
pub fn clear_target(mut state: AppState) {
    if state.drag.peek().as_ref().is_some_and(|d| d.target.is_some())
        && let Some(d) = state.drag.write().as_mut()
    {
        d.target = None;
    }
}

/// A drop zone was entered. Targets that don't fit the dragged item are ignored.
pub fn set_target(mut state: AppState, target: DropTarget) {
    let fits = match state.drag.peek().as_ref() {
        Some(d) if d.active => match (d.item, target) {
            (DragItem::Channel(c), DropTarget::BeforeChannel(t) | DropTarget::AfterChannel(t)) => c != t,
            (DragItem::Channel(_), DropTarget::IntoCategory(_)) => true,
            (DragItem::Category(c), DropTarget::BeforeCategory(t) | DropTarget::AfterCategory(t)) => c != t,
            (DragItem::Member { from, .. }, DropTarget::Voice(t)) => from != t,
            _ => false,
        },
        _ => false,
    };
    let target = fits.then_some(target);
    if state.drag.peek().as_ref().is_some_and(|d| d.active && d.target != target)
        && let Some(d) = state.drag.write().as_mut()
    {
        d.target = target;
    }
}

/// Mouse released: drop if there's a target, otherwise just stop.
pub fn finish(mut state: AppState) {
    let Some(drag) = state.drag.take() else { return };
    if !drag.active {
        return;
    }
    ENDED.with(|c| c.set(Some(std::time::Instant::now())));
    if let Some(target) = drag.target {
        drop_on(state, &drag, target);
    }
}

/// Whether the current drag is active (rows show their drop zones).
pub fn active(state: &AppState) -> Option<DragItem> {
    state.drag.read().as_ref().filter(|d| d.active).map(|d| d.item)
}

pub fn target(state: &AppState) -> Option<DropTarget> {
    state.drag.read().as_ref().filter(|d| d.active).and_then(|d| d.target)
}

fn drop_on(state: AppState, drag: &Drag, target: DropTarget) {
    match (drag.item, target) {
        (DragItem::Member { user, .. }, DropTarget::Voice(channel)) => {
            if Some(user) == state.me() {
                super::call::join_channel(state, channel);
                return;
            }
            let Some(s) = state.session.cloned() else { return };
            let guild = drag.guild;
            spawn_forever(async move {
                state.report(s.api.move_voice_member(guild, user, channel).await.map_err(|e| e.to_string()));
            });
        }
        (item, target) => reorder(state, drag.guild, item, target),
    }
}

/// Moves a channel or category and saves the new channel list.
fn reorder(mut state: AppState, guild: GuildId, item: DragItem, target: DropTarget) {
    let Some(g) = state.guilds.peek().iter().find(|g| g.guild.id == guild).cloned() else { return };
    let mut categories = g.categories.clone();
    categories.sort_by_key(|c| c.position);
    let mut order: Vec<CategoryId> = categories.iter().map(|c| c.id).collect();
    // Channel lists as shown: loose ones first, then each category's.
    let mut lists: Vec<(Option<CategoryId>, Vec<Channel>)> =
        std::iter::once(None).chain(order.iter().copied().map(Some)).map(|c| (c, Vec::new())).collect();
    let mut channels = g.channels.clone();
    channels.sort_by_key(|c| (c.kind == ChannelKind::Voice, c.position));
    for c in channels {
        if let Some(list) = lists.iter_mut().find(|(cat, _)| *cat == c.category_id) {
            list.1.push(c);
        }
    }

    match item {
        DragItem::Channel(id) => {
            let Some(moved) = lists.iter_mut().find_map(|(_, l)| {
                let i = l.iter().position(|c| c.id == id)?;
                Some(l.remove(i))
            }) else {
                return;
            };
            let (list, index) = match target {
                DropTarget::IntoCategory(cat) => (lists.iter().position(|(c, _)| *c == Some(cat)), Some(0)),
                DropTarget::BeforeChannel(t) | DropTarget::AfterChannel(t) => {
                    let list = lists.iter().position(|(_, l)| l.iter().any(|c| c.id == t));
                    let index = list.and_then(|i| lists[i].1.iter().position(|c| c.id == t));
                    (list, index.map(|i| if matches!(target, DropTarget::AfterChannel(_)) { i + 1 } else { i }))
                }
                _ => return,
            };
            let (Some(list), Some(index)) = (list, index) else { return };
            let mut moved = moved;
            moved.category_id = lists[list].0;
            lists[list].1.insert(index, moved);
            // Text channels stay above voice channels within a category.
            lists[list].1.sort_by_key(|c| c.kind == ChannelKind::Voice);
        }
        DragItem::Category(id) => {
            let Some(from) = order.iter().position(|c| *c == id) else { return };
            order.remove(from);
            let (t, after) = match target {
                DropTarget::BeforeCategory(t) => (t, false),
                DropTarget::AfterCategory(t) => (t, true),
                _ => return,
            };
            let Some(i) = order.iter().position(|c| *c == t) else { return };
            order.insert(if after { i + 1 } else { i }, id);
            lists.sort_by_key(|(c, _)| c.map_or(-1, |c| order.iter().position(|o| *o == c).unwrap_or(0) as i64));
        }
        DragItem::Member { .. } => return,
    }

    let placements: Vec<ChannelPlacement> = lists
        .iter()
        .flat_map(|(cat, l)| l.iter().map(move |c| ChannelPlacement { id: c.id, category_id: *cat }))
        .collect();
    // Show it right away; the server confirms with a guild update.
    if let Some(local) = state.guilds.write().iter_mut().find(|g| g.guild.id == guild) {
        for (i, p) in placements.iter().enumerate() {
            if let Some(c) = local.channels.iter_mut().find(|c| c.id == p.id) {
                c.position = i as i32;
                c.category_id = p.category_id;
            }
        }
        for (i, id) in order.iter().enumerate() {
            if let Some(c) = local.categories.iter_mut().find(|c| c.id == *id) {
                c.position = i as i32;
            }
        }
    }
    let Some(s) = state.session.cloned() else { return };
    let req = LayoutRequest { categories: order, channels: placements };
    spawn_forever(async move {
        state.report(s.api.set_layout(guild, &req).await.map_err(|e| e.to_string()));
    });
}

/// The dragged item, half transparent next to the mouse.
#[component]
pub fn DragGhost() -> Element {
    let state = use_context::<AppState>();
    let Some(drag) = state.drag.read().clone().filter(|d| d.active) else { return rsx! {} };
    // Below and right of the pointer: Blitz has no `pointer-events`, so the
    // ghost must not cover the drop zones under the mouse.
    let style = format!("left: {}px; top: {}px;", drag.pos.0 + 14.0, drag.pos.1 + 10.0);
    let guild = Some(drag.guild);
    match drag.item {
        DragItem::Channel(id) => {
            let Some(c) = state.find_channel(id) else { return rsx! {} };
            rsx! {
                div { class: "drag-ghost channel", style: "{style}",
                    if c.kind == ChannelKind::Voice { Svg { icon: Icon::Volume, size: 18 } } else { Svg { icon: Icon::Hash, size: 18 } }
                    span { class: "channel-name", "{c.name}" }
                }
            }
        }
        DragItem::Category(id) => {
            let name = state
                .guilds
                .read()
                .iter()
                .flat_map(|g| g.categories.iter())
                .find(|c| c.id == id)
                .map(|c| c.name.clone())
                .unwrap_or_default();
            rsx! {
                div { class: "drag-ghost category", style: "{style}",
                    Svg { icon: Icon::ChevronDown, size: 12 }
                    "{name}"
                }
            }
        }
        DragItem::Member { user, .. } => {
            let name = state.display_name_in(guild, user);
            rsx! {
                div { class: "drag-ghost voice-member", style: "{style}",
                    Avatar { name: name.clone(), id: Some(user), size: 22 }
                    span { class: "voice-member-name", "{name}" }
                }
            }
        }
    }
}
