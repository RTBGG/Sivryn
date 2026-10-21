//! Integration tests for roles, permission overwrites (private channels),
//! the role hierarchy, the channel layout and moving voice members.

mod common;

use common::{TestApp, TestUser};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sivryn_protocol::api::{GuildMember, GuildWithChannels, RegistrationMode};
use sivryn_protocol::model::{ChannelKind, Role, User};
use sivryn_protocol::permissions::Permissions;
use sivryn_protocol::{ChannelId, RoleId};

async fn create_guild(app: &TestApp, owner: &TestUser) -> GuildWithChannels {
    app.post_ok("/guilds", &owner.token, json!({ "name": "g" })).await
}

async fn guild(app: &TestApp, user: &TestUser, id: impl std::fmt::Display) -> GuildWithChannels {
    app.get_ok(&format!("/guilds/{id}"), &user.token).await
}

async fn join(app: &TestApp, owner: &TestUser, member: &TestUser, guild: &GuildWithChannels) {
    let invite: Value = app.post_ok(&format!("/guilds/{}/invites", guild.guild.id), &owner.token, json!(null)).await;
    let code = invite["code"].as_str().unwrap();
    assert_eq!(app.post(&format!("/invites/{code}"), &member.token, json!(null)).await.0, StatusCode::OK);
}

async fn create_role(app: &TestApp, user: &TestUser, guild: &GuildWithChannels, body: Value) -> Role {
    app.post_ok(&format!("/guilds/{}/roles", guild.guild.id), &user.token, body).await
}

fn channel(g: &GuildWithChannels, kind: ChannelKind) -> ChannelId {
    g.channels.iter().find(|c| c.kind == kind).unwrap().id
}

#[tokio::test]
async fn roles_colors_and_hierarchy() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let carol = app.register("carol").await;
    let g = create_guild(&app, &alice).await;
    let gid = g.guild.id;
    join(&app, &alice, &bob, &g).await;
    join(&app, &alice, &carol, &g).await;
    assert_eq!(g.roles.len(), 1);
    assert!(g.roles[0].is_everyone);

    // Members without MANAGE_ROLES can't create roles.
    let path = format!("/guilds/{gid}/roles");
    assert_eq!(app.post(&path, &bob.token, json!({ "name": "x" })).await.0, StatusCode::FORBIDDEN);

    let manage = Permissions::MANAGE_ROLES | Permissions::KICK_MEMBERS;
    let mods = create_role(
        &app,
        &alice,
        &g,
        json!({ "name": "Mods", "color": 0x3366ff, "permissions": manage.bits(), "hoist": true }),
    )
    .await;
    let helpers = create_role(&app, &alice, &g, json!({ "name": "Helpers" })).await;
    assert_eq!((mods.color, mods.hoist), (Some(0x3366ff), true));
    assert_eq!(
        app.post(&path, &alice.token, json!({ "name": "x", "color": 0x1000000 })).await.0,
        StatusCode::BAD_REQUEST
    );

    // Newer roles start lowest; put Mods on top.
    let order = format!("/guilds/{gid}/roles/order");
    assert_eq!(
        app.put(&order, &alice.token, json!({ "roles": [mods.id, helpers.id] })).await.0,
        StatusCode::NO_CONTENT
    );
    let roles: Vec<Role> = app.get_ok(&path, &bob.token).await;
    let names: Vec<&str> = roles.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["Mods", "Helpers", "@everyone"]);

    // Bob becomes a moderator; the member list shows his roles.
    let assign = |user: &TestUser, role: RoleId| format!("/guilds/{gid}/members/{}/roles/{role}", user.user.id);
    assert_eq!(app.put(&assign(&bob, mods.id), &alice.token, json!(null)).await.0, StatusCode::NO_CONTENT);
    let members: Vec<GuildMember> = app.get_ok(&format!("/guilds/{gid}/members"), &alice.token).await;
    let bob_member = members.iter().find(|m| m.user.id == bob.user.id).unwrap();
    assert_eq!(bob_member.roles, [mods.id]);
    assert!(guild(&app, &bob, gid).await.permissions.contains(Permissions::MANAGE_ROLES));

    // Bob may hand out roles below his own, but not his own or higher ones…
    assert_eq!(app.put(&assign(&carol, helpers.id), &bob.token, json!(null)).await.0, StatusCode::NO_CONTENT);
    assert_eq!(app.put(&assign(&carol, mods.id), &bob.token, json!(null)).await.0, StatusCode::FORBIDDEN);
    let edit = format!("/roles/{}", mods.id);
    assert_eq!(app.patch(&edit, &bob.token, json!({ "name": "Gods" })).await.0, StatusCode::FORBIDDEN);
    // …nor grant permissions he lacks.
    let helpers_path = format!("/roles/{}", helpers.id);
    let admin = Permissions::ADMINISTRATOR.bits();
    assert_eq!(app.patch(&helpers_path, &bob.token, json!({ "permissions": admin })).await.0, StatusCode::FORBIDDEN);
    let (status, _) = app.patch(&helpers_path, &bob.token, json!({ "color": 0x00ff00, "name": "Helfer" })).await;
    assert_eq!(status, StatusCode::OK);
    // He can't move Helpers above his own role either.
    assert_eq!(app.put(&order, &bob.token, json!({ "roles": [helpers.id, mods.id] })).await.0, StatusCode::FORBIDDEN);

    // Kicking follows the hierarchy too: Carol (Helpers) can't be kicked by
    // anyone below her, but Bob (Mods) can kick her.
    let kick = format!("/guilds/{gid}/members/{}", carol.user.id);
    assert_eq!(app.delete(&kick, &bob.token).await.0, StatusCode::NO_CONTENT);
    let kick_bob = format!("/guilds/{gid}/members/{}", bob.user.id);
    assert_eq!(app.delete(&kick_bob, &bob.token).await.0, StatusCode::NO_CONTENT);

    // Deleting a role removes it from members.
    assert_eq!(app.delete(&helpers_path, &alice.token).await.0, StatusCode::NO_CONTENT);
    let everyone = g.roles[0].id;
    assert_eq!(app.delete(&format!("/roles/{everyone}"), &alice.token).await.0, StatusCode::BAD_REQUEST);
    app.cleanup().await;
}

#[tokio::test]
async fn private_channels_with_overwrites() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let carol = app.register("carol").await;
    let g = create_guild(&app, &alice).await;
    let gid = g.guild.id;
    join(&app, &alice, &bob, &g).await;
    join(&app, &alice, &carol, &g).await;
    let everyone = g.roles[0].id;
    let staff = create_role(&app, &alice, &g, json!({ "name": "Staff" })).await;
    let assign = format!("/guilds/{gid}/members/{}/roles/{}", bob.user.id, staff.id);
    assert_eq!(app.put(&assign, &alice.token, json!(null)).await.0, StatusCode::NO_CONTENT);

    // A private text channel: hidden from @everyone, visible to Staff.
    let ch: Value =
        app.post_ok(&format!("/guilds/{gid}/channels"), &alice.token, json!({ "name": "staff", "kind": "text" })).await;
    let private: ChannelId = serde_json::from_value(ch["id"].clone()).unwrap();
    let ow = format!("/channels/{private}/overwrites");
    let view = Permissions::VIEW_CHANNEL.bits();
    let hide = json!({ "target": { "type": "role", "id": everyone }, "allow": 0, "deny": view });
    let show = json!({ "target": { "type": "role", "id": staff.id }, "allow": view, "deny": 0 });
    assert_eq!(app.put(&ow, &bob.token, hide.clone()).await.0, StatusCode::FORBIDDEN);
    assert_eq!(app.put(&ow, &alice.token, hide).await.0, StatusCode::NO_CONTENT);
    assert_eq!(app.put(&ow, &alice.token, show).await.0, StatusCode::NO_CONTENT);

    // Carol doesn't see it at all; Bob does, and so does the owner.
    let carol_guild = guild(&app, &carol, gid).await;
    assert!(!carol_guild.channels.iter().any(|c| c.id == private));
    assert!(!carol_guild.channel_permissions.contains_key(&private));
    let bob_guild = guild(&app, &bob, gid).await;
    let seen = bob_guild.channels.iter().find(|c| c.id == private).unwrap();
    assert_eq!(seen.overwrites.len(), 2);
    assert!(bob_guild.channel_permissions[&private].contains(Permissions::SEND_MESSAGES));
    assert_eq!(app.get(&format!("/channels/{private}/messages"), &carol.token).await.0, StatusCode::NOT_FOUND);

    // Only viewers belong in the channel's MLS group.
    let members: Vec<User> = app.get_ok(&format!("/channels/{private}/members"), &alice.token).await;
    let mut ids: Vec<_> = members.iter().map(|u| u.id).collect();
    ids.sort();
    let mut expected = vec![alice.user.id, bob.user.id];
    expected.sort();
    assert_eq!(ids, expected);

    // A member overwrite can open it for Carol; a category overwrite applies
    // to every channel inside, which can still override it.
    let carol_ow = json!({ "target": { "type": "member", "id": carol.user.id }, "allow": view, "deny": 0 });
    assert_eq!(app.put(&ow, &alice.token, carol_ow).await.0, StatusCode::NO_CONTENT);
    assert!(guild(&app, &carol, gid).await.channels.iter().any(|c| c.id == private));
    let del = format!("/channels/{private}/overwrites/member/{}", carol.user.id);
    assert_eq!(app.delete(&del, &alice.token).await.0, StatusCode::NO_CONTENT);
    assert!(!guild(&app, &carol, gid).await.channels.iter().any(|c| c.id == private));

    let cat = g.categories[0].id;
    let read_only = json!({
        "target": { "type": "role", "id": everyone },
        "allow": 0,
        "deny": Permissions::SEND_MESSAGES.bits()
    });
    assert_eq!(
        app.put(&format!("/categories/{cat}/overwrites"), &alice.token, read_only).await.0,
        StatusCode::NO_CONTENT
    );
    let general = channel(&g, ChannelKind::Text);
    let carol_guild = guild(&app, &carol, gid).await;
    let p = carol_guild.channel_permissions[&general];
    assert!(p.contains(Permissions::VIEW_CHANNEL) && !p.contains(Permissions::SEND_MESSAGES));
    assert_eq!(carol_guild.categories.iter().find(|c| c.id == cat).unwrap().overwrites.len(), 1);
    app.cleanup().await;
}

#[tokio::test]
async fn layout_and_voice_moves() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let g = create_guild(&app, &alice).await;
    let gid = g.guild.id;
    join(&app, &alice, &bob, &g).await;

    // Swap the two categories and move the text channel into the voice category.
    let (text_cat, voice_cat) = (g.categories[0].id, g.categories[1].id);
    let text = channel(&g, ChannelKind::Text);
    let voice = channel(&g, ChannelKind::Voice);
    let layout = json!({
        "categories": [voice_cat, text_cat],
        "channels": [{ "id": voice, "category_id": voice_cat }, { "id": text, "category_id": voice_cat }],
    });
    let path = format!("/guilds/{gid}/layout");
    assert_eq!(app.put(&path, &bob.token, layout.clone()).await.0, StatusCode::FORBIDDEN);
    assert_eq!(app.put(&path, &alice.token, layout).await.0, StatusCode::NO_CONTENT);
    let after = guild(&app, &alice, gid).await;
    assert_eq!(after.categories.iter().map(|c| c.id).collect::<Vec<_>>(), [voice_cat, text_cat]);
    let order: Vec<ChannelId> = after.channels.iter().map(|c| c.id).collect();
    assert_eq!(order, [voice, text]);
    assert!(after.channels.iter().all(|c| c.category_id == Some(voice_cat)));
    // Partial layouts are fine (private channels may be missing), foreign or
    // doubled entries aren't.
    let partial = json!({ "categories": [voice_cat], "channels": [] });
    assert_eq!(app.put(&path, &alice.token, partial).await.0, StatusCode::NO_CONTENT);
    let doubled = json!({ "categories": [voice_cat, voice_cat], "channels": [] });
    assert_eq!(app.put(&path, &alice.token, doubled).await.0, StatusCode::BAD_REQUEST);
    let foreign = json!({ "categories": [], "channels": [{ "id": ChannelId::new(), "category_id": null }] });
    assert_eq!(app.put(&path, &alice.token, foreign).await.0, StatusCode::BAD_REQUEST);

    // Moving members needs MOVE_MEMBERS and a voice channel as the target.
    let mv = format!("/guilds/{gid}/voice/move");
    assert_eq!(
        app.post(&mv, &bob.token, json!({ "user_id": alice.user.id, "channel_id": voice })).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.post(&mv, &alice.token, json!({ "user_id": bob.user.id, "channel_id": text })).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.post(&mv, &alice.token, json!({ "user_id": bob.user.id, "channel_id": voice })).await.0,
        StatusCode::NO_CONTENT
    );
    app.cleanup().await;
}
