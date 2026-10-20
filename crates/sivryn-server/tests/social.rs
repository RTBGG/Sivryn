//! Integration tests for friends, DMs, moderation, message actions and files.

mod common;

use common::{TestApp, TestUser};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sivryn_protocol::api::{
    Ban, DmChannel, FriendsResponse, GuildWithChannels, RegisterDeviceResponse, RegistrationMode, UserProfile,
};
use sivryn_protocol::model::{ChannelKind, Guild, Message, User};
use sivryn_protocol::{ChannelId, DeviceId};

async fn create_guild(app: &TestApp, owner: &TestUser, name: &str) -> GuildWithChannels {
    app.post_ok("/guilds", &owner.token, json!({ "name": name })).await
}

async fn join_guild(app: &TestApp, owner: &TestUser, member: &TestUser, guild: &GuildWithChannels) -> StatusCode {
    let invite: Value = app.post_ok(&format!("/guilds/{}/invites", guild.guild.id), &owner.token, json!(null)).await;
    let code = invite["code"].as_str().unwrap();
    app.post(&format!("/invites/{code}"), &member.token, json!(null)).await.0
}

async fn device(app: &TestApp, user: &TestUser) -> DeviceId {
    let resp: RegisterDeviceResponse =
        app.post_ok("/devices", &user.token, json!({ "name": "test", "key_packages": [] })).await;
    resp.device_id
}

async fn friends(app: &TestApp, user: &TestUser) -> FriendsResponse {
    app.get_ok("/friends", &user.token).await
}

fn names(users: &[User]) -> Vec<&str> {
    users.iter().map(|u| u.username.as_str()).collect()
}

async fn send(app: &TestApp, user: &TestUser, dev: DeviceId, channel: ChannelId, body: Value) -> (StatusCode, Value) {
    let mut req = json!({ "device_id": dev, "epoch": 0, "ciphertext": "ct" });
    if let (Some(req), Some(extra)) = (req.as_object_mut(), body.as_object()) {
        req.extend(extra.clone());
    }
    app.post(&format!("/channels/{channel}/messages"), &user.token, req).await
}

#[tokio::test]
async fn friend_requests_accept_decline_and_block() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let carol = app.register("carol").await;

    assert_eq!(
        app.post("/friends/requests", &alice.token, json!({ "username": "nobody" })).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.post("/friends/requests", &alice.token, json!({ "username": "ALICE" })).await.0,
        StatusCode::BAD_REQUEST
    );

    // Request (case-insensitive name), then accept.
    assert_eq!(
        app.post("/friends/requests", &alice.token, json!({ "username": "Bob" })).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(names(&friends(&app, &alice).await.outgoing), ["bob"]);
    assert_eq!(names(&friends(&app, &bob).await.incoming), ["alice"]);
    let accept = format!("/friends/requests/{}/accept", alice.user.id);
    assert_eq!(app.post(&accept, &bob.token, json!(null)).await.0, StatusCode::NO_CONTENT);
    assert_eq!(names(&friends(&app, &alice).await.friends), ["bob"]);
    assert_eq!(names(&friends(&app, &bob).await.friends), ["alice"]);
    assert_eq!(app.post("/friends/requests", &alice.token, json!({ "username": "bob" })).await.0, StatusCode::CONFLICT);

    // Asking someone who already asked you makes you friends right away.
    app.post("/friends/requests", &carol.token, json!({ "username": "alice" })).await;
    app.post("/friends/requests", &alice.token, json!({ "username": "carol" })).await;
    assert_eq!(names(&friends(&app, &alice).await.friends), ["bob", "carol"]);

    // Profiles show the relationship.
    let profile: UserProfile = app.get_ok(&format!("/users/{}", bob.user.id), &alice.token).await;
    assert_eq!(profile.relationship, sivryn_protocol::api::Relationship::Friend);

    // Blocking ends the friendship and hides you from their requests.
    assert_eq!(
        app.put(&format!("/blocks/{}", carol.user.id), &alice.token, json!(null)).await.0,
        StatusCode::NO_CONTENT
    );
    let a = friends(&app, &alice).await;
    assert_eq!(names(&a.friends), ["bob"]);
    assert_eq!(names(&a.blocked), ["carol"]);
    assert_eq!(
        app.post("/friends/requests", &carol.token, json!({ "username": "alice" })).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(app.delete(&format!("/blocks/{}", carol.user.id), &alice.token).await.0, StatusCode::NO_CONTENT);
    assert!(friends(&app, &alice).await.blocked.is_empty());

    // Remove a friend; declining a request.
    assert_eq!(app.delete(&format!("/friends/{}", bob.user.id), &alice.token).await.0, StatusCode::NO_CONTENT);
    assert!(friends(&app, &bob).await.friends.is_empty());
    app.post("/friends/requests", &carol.token, json!({ "username": "bob" })).await;
    assert_eq!(app.delete(&format!("/friends/requests/{}", carol.user.id), &bob.token).await.0, StatusCode::NO_CONTENT);
    assert!(friends(&app, &carol).await.outgoing.is_empty());

    app.cleanup().await;
}

#[tokio::test]
async fn dms_groups_and_who_may_message() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let carol = app.register("carol").await;
    let dave = app.register("dave").await;

    // Strangers can't DM each other.
    let open = |ids: Vec<&User>| json!({ "user_ids": ids.iter().map(|u| u.id).collect::<Vec<_>>() });
    assert_eq!(app.post("/dms", &alice.token, open(vec![&dave.user])).await.0, StatusCode::FORBIDDEN);

    // Friends can; opening again reuses the DM.
    app.post("/friends/requests", &alice.token, json!({ "username": "bob" })).await;
    app.post(&format!("/friends/requests/{}/accept", alice.user.id), &bob.token, json!(null)).await;
    let dm: DmChannel = app.post_ok("/dms", &alice.token, open(vec![&bob.user])).await;
    assert_eq!(dm.channel.kind, ChannelKind::Dm);
    assert_eq!(dm.channel.guild_id, None);
    assert_eq!(names(&dm.members), ["alice", "bob"]);
    let again: DmChannel = app.post_ok("/dms", &bob.token, open(vec![&alice.user])).await;
    assert_eq!(again.channel.id, dm.channel.id);

    // Members talk; outsiders can't read.
    let a_dev = device(&app, &alice).await;
    assert_eq!(send(&app, &alice, a_dev, dm.channel.id, json!({})).await.0, StatusCode::CREATED);
    let path = format!("/channels/{}/messages", dm.channel.id);
    assert_eq!(app.get(&path, &carol.token).await.0, StatusCode::NOT_FOUND);
    let page: Value = app.get_ok(&path, &bob.token).await;
    assert_eq!(page["messages"].as_array().unwrap().len(), 1);
    let members: Vec<User> = app.get_ok(&format!("/channels/{}/members", dm.channel.id), &bob.token).await;
    assert_eq!(names(&members), ["alice", "bob"]);

    // Sharing a server is enough, unless the user turned server DMs off.
    let guild = create_guild(&app, &alice, "g").await;
    join_guild(&app, &alice, &carol, &guild).await;
    let (status, _) = app.post("/dms", &alice.token, open(vec![&carol.user])).await;
    assert!(status.is_success());
    app.patch("/users/@me", &carol.token, json!({ "allow_server_dms": false })).await;
    join_guild(&app, &alice, &dave, &guild).await;
    assert_eq!(app.post("/dms", &dave.token, open(vec![&carol.user])).await.0, StatusCode::FORBIDDEN);
    app.patch("/users/@me", &carol.token, json!({ "allow_server_dms": true })).await;

    // Adding someone to a 1:1 DM creates a new group; the DM stays.
    let group: DmChannel = app
        .post_ok(&format!("/dms/{}/members", dm.channel.id), &alice.token, json!({ "user_id": carol.user.id }))
        .await;
    assert_eq!(group.channel.kind, ChannelKind::Group);
    assert_ne!(group.channel.id, dm.channel.id);
    assert_eq!(names(&group.members), ["alice", "bob", "carol"]);
    // Groups grow in place, but only with people the adder may message.
    let add_dave = json!({ "user_id": dave.user.id });
    let members_path = format!("/dms/{}/members", group.channel.id);
    assert_eq!(app.post(&members_path, &bob.token, add_dave.clone()).await.0, StatusCode::FORBIDDEN);
    let grown: DmChannel = app.post_ok(&members_path, &alice.token, add_dave).await;
    assert_eq!(grown.channel.id, group.channel.id);
    assert_eq!(grown.members.len(), 4);

    // Renaming and leaving.
    let renamed: DmChannel = serde_json::from_value(
        app.patch(&format!("/dms/{}", group.channel.id), &carol.token, json!({ "name": "Squad" })).await.1,
    )
    .unwrap();
    assert_eq!(renamed.channel.name, "Squad");
    assert_eq!(
        app.delete(&format!("/dms/{}/members/@me", group.channel.id), &dave.token).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.get(&format!("/channels/{}/messages", group.channel.id), &dave.token).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        app.delete(&format!("/dms/{}/members/@me", dm.channel.id), &alice.token).await.0,
        StatusCode::BAD_REQUEST
    );

    let list: Vec<DmChannel> = app.get_ok("/dms", &alice.token).await;
    assert_eq!(list.len(), 3);

    // DM calls work for members only.
    assert!(app.post(&format!("/channels/{}/voice/join", dm.channel.id), &bob.token, json!(null)).await.0.is_success());
    assert_eq!(
        app.post(&format!("/channels/{}/voice/join", dm.channel.id), &dave.token, json!(null)).await.0,
        StatusCode::NOT_FOUND
    );

    app.cleanup().await;
}

#[tokio::test]
async fn guild_settings_kick_ban_and_unban() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let owner = app.register("owner").await;
    let bob = app.register("bob").await;
    let guild = create_guild(&app, &owner, "Before").await;
    let gid = guild.guild.id;
    assert_eq!(join_guild(&app, &owner, &bob, &guild).await, StatusCode::OK);
    let voice = guild.channels.iter().find(|c| c.kind == ChannelKind::Voice).unwrap().id;
    let text = guild.channels.iter().find(|c| c.kind == ChannelKind::Text).unwrap().id;

    // Settings need MANAGE_GUILD.
    let path = format!("/guilds/{gid}");
    assert_eq!(app.patch(&path, &bob.token, json!({ "name": "x" })).await.0, StatusCode::FORBIDDEN);
    let (status, json) = app
        .patch(&path, &owner.token, json!({ "name": "After", "afk_channel_id": voice, "afk_timeout_minutes": 5 }))
        .await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let updated: Guild = serde_json::from_value(json).unwrap();
    assert_eq!((updated.name.as_str(), updated.afk_channel_id, updated.afk_timeout_minutes), ("After", Some(voice), 5));
    assert_eq!(app.patch(&path, &owner.token, json!({ "afk_channel_id": text })).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(app.patch(&path, &owner.token, json!({ "afk_timeout_minutes": 7 })).await.0, StatusCode::BAD_REQUEST);
    let cleared: Guild =
        serde_json::from_value(app.patch(&path, &owner.token, json!({ "afk_channel_id": null })).await.1).unwrap();
    assert_eq!(cleared.afk_channel_id, None);

    // Icon: images only, readable by members.
    let png = b"\x89PNG\r\n\x1a\nfake".to_vec();
    let icon_path = format!("/guilds/{gid}/icon");
    assert_eq!(
        app.send_bytes(Method::PUT, &icon_path, &owner.token, "text/plain", png.clone()).await.0,
        StatusCode::BAD_REQUEST
    );
    let (status, json) = app.send_bytes(Method::PUT, &icon_path, &owner.token, "image/png", png.clone()).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let icon = json["id"].as_str().unwrap().to_owned();
    let (status, bytes) = app.get_bytes(&format!("/files/{icon}"), &bob.token).await;
    assert_eq!((status, bytes), (StatusCode::OK, png));

    // Kick: needs KICK_MEMBERS; the owner can't be kicked or leave.
    let kick = format!("/guilds/{gid}/members/{}", bob.user.id);
    assert_eq!(
        app.delete(&format!("/guilds/{gid}/members/{}", owner.user.id), &bob.token).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.delete(&format!("/guilds/{gid}/members/{}", owner.user.id), &owner.token).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(app.delete(&kick, &owner.token).await.0, StatusCode::NO_CONTENT);
    assert_eq!(app.get(&path, &bob.token).await.0, StatusCode::NOT_FOUND);
    assert_eq!(app.get(&format!("/files/{icon}"), &bob.token).await.0, StatusCode::NOT_FOUND);

    // Ban: removes the member and blocks rejoining until unbanned.
    assert_eq!(join_guild(&app, &owner, &bob, &guild).await, StatusCode::OK);
    let ban = format!("/guilds/{gid}/bans/{}", bob.user.id);
    assert_eq!(app.put(&ban, &bob.token, json!({})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(app.put(&ban, &owner.token, json!({ "reason": "spam" })).await.0, StatusCode::NO_CONTENT);
    assert_eq!(app.get(&path, &bob.token).await.0, StatusCode::NOT_FOUND);
    assert_eq!(join_guild(&app, &owner, &bob, &guild).await, StatusCode::FORBIDDEN);
    let bans: Vec<Ban> = app.get_ok(&format!("/guilds/{gid}/bans"), &owner.token).await;
    assert_eq!((bans[0].user.id, bans[0].reason.as_deref()), (bob.user.id, Some("spam")));
    assert_eq!(app.delete(&ban, &owner.token).await.0, StatusCode::NO_CONTENT);
    assert_eq!(join_guild(&app, &owner, &bob, &guild).await, StatusCode::OK);

    // Leaving on your own.
    assert_eq!(app.delete(&kick, &bob.token).await.0, StatusCode::NO_CONTENT);

    app.cleanup().await;
}

#[tokio::test]
async fn edits_reactions_and_deletes() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let guild = create_guild(&app, &alice, "g").await;
    join_guild(&app, &alice, &bob, &guild).await;
    let channel = guild.channels.iter().find(|c| c.kind == ChannelKind::Text).unwrap().id;
    let (a_dev, b_dev) = (device(&app, &alice).await, device(&app, &bob).await);

    let (_, json) = send(&app, &bob, b_dev, channel, json!({})).await;
    let original: Message = serde_json::from_value(json).unwrap();

    // Only the author edits; anyone reacts; targets must be real messages.
    let edit = json!({ "kind": "edit", "target_id": original.id });
    assert_eq!(send(&app, &alice, a_dev, channel, edit.clone()).await.0, StatusCode::FORBIDDEN);
    assert_eq!(send(&app, &bob, b_dev, channel, edit).await.0, StatusCode::CREATED);
    let (status, json) =
        send(&app, &alice, a_dev, channel, json!({ "kind": "reaction", "target_id": original.id })).await;
    assert_eq!(status, StatusCode::CREATED);
    let reaction: Message = serde_json::from_value(json).unwrap();
    assert_eq!(send(&app, &alice, a_dev, channel, json!({ "kind": "reaction" })).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(
        send(&app, &alice, a_dev, channel, json!({ "kind": "reaction", "target_id": reaction.id })).await.0,
        StatusCode::BAD_REQUEST
    );

    // Others can't delete without MANAGE_MESSAGES; deleting takes edits and reactions along.
    let del = format!("/channels/{channel}/messages/{}", original.id);
    let (_, json) = send(&app, &alice, a_dev, channel, json!({})).await;
    let alices: Message = serde_json::from_value(json).unwrap();
    assert_eq!(
        app.delete(&format!("/channels/{channel}/messages/{}", alices.id), &bob.token).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(app.delete(&del, &bob.token).await.0, StatusCode::NO_CONTENT);
    let page: Value = app.get_ok(&format!("/channels/{channel}/messages"), &alice.token).await;
    let ids: Vec<&str> = page["messages"].as_array().unwrap().iter().map(|m| m["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [alices.id.to_string()]);
    // The guild owner moderates.
    let (_, json) = send(&app, &bob, b_dev, channel, json!({})).await;
    let bobs: Message = serde_json::from_value(json).unwrap();
    assert_eq!(
        app.delete(&format!("/channels/{channel}/messages/{}", bobs.id), &alice.token).await.0,
        StatusCode::NO_CONTENT
    );

    app.cleanup().await;
}

#[tokio::test]
async fn attachments_respect_channel_access_and_size_limit() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let mallory = app.register("mallory").await;
    let guild = create_guild(&app, &alice, "g").await;
    let channel = guild.channels.iter().find(|c| c.kind == ChannelKind::Text).unwrap().id;
    let path = format!("/channels/{channel}/attachments");

    let data: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
    assert_eq!(
        app.send_bytes(Method::POST, &path, &mallory.token, "application/octet-stream", data.clone()).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, json) =
        app.send_bytes(Method::POST, &path, &alice.token, "application/octet-stream", data.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let file = json["id"].as_str().unwrap().to_owned();
    assert_eq!(json["size"], 5000);
    assert_eq!(app.get_bytes(&format!("/files/{file}"), &alice.token).await, (StatusCode::OK, data));
    assert_eq!(app.get(&format!("/files/{file}"), &mallory.token).await.0, StatusCode::NOT_FOUND);

    // The test server allows 1 MB.
    let big = vec![0u8; 1024 * 1024 + 1];
    let (status, _) = app.send_bytes(Method::POST, &path, &alice.token, "application/octet-stream", big).await;
    assert!(status == StatusCode::BAD_REQUEST || status == StatusCode::PAYLOAD_TOO_LARGE, "{status}");

    app.cleanup().await;
}
