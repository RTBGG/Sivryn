//! Integration tests for categories, channel management, read states,
//! mutes, member settings, profiles and status.

mod common;

use common::{TestApp, TestUser};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use sivryn_protocol::api::{
    ChannelMute, GuildMember, GuildWithChannels, MeSettings, ReadState, RegisterDeviceResponse, RegistrationMode,
    UserPresence, UserProfile,
};
use sivryn_protocol::model::{ChannelKind, Message, NotifyLevel, User, UserStatus};
use sivryn_protocol::{ChannelId, DeviceId};

async fn create_guild(app: &TestApp, owner: &TestUser) -> GuildWithChannels {
    app.post_ok("/guilds", &owner.token, json!({ "name": "g", "text_category": "Text", "voice_category": "Sprache" }))
        .await
}

async fn guild(app: &TestApp, user: &TestUser, id: impl std::fmt::Display) -> GuildWithChannels {
    app.get_ok(&format!("/guilds/{id}"), &user.token).await
}

async fn join(app: &TestApp, owner: &TestUser, member: &TestUser, guild: &GuildWithChannels) {
    let invite: Value = app.post_ok(&format!("/guilds/{}/invites", guild.guild.id), &owner.token, json!(null)).await;
    let code = invite["code"].as_str().unwrap();
    assert_eq!(app.post(&format!("/invites/{code}"), &member.token, json!(null)).await.0, StatusCode::OK);
}

async fn device(app: &TestApp, user: &TestUser) -> DeviceId {
    let resp: RegisterDeviceResponse =
        app.post_ok("/devices", &user.token, json!({ "name": "test", "key_packages": [] })).await;
    resp.device_id
}

async fn send(app: &TestApp, user: &TestUser, dev: DeviceId, channel: ChannelId) -> Message {
    app.post_ok(
        &format!("/channels/{channel}/messages"),
        &user.token,
        json!({ "device_id": dev, "epoch": 0, "ciphertext": "ct" }),
    )
    .await
}

fn text_channel(g: &GuildWithChannels) -> ChannelId {
    g.channels.iter().find(|c| c.kind == ChannelKind::Text).unwrap().id
}

#[tokio::test]
async fn categories_and_channel_management() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let g = create_guild(&app, &alice).await;
    let gid = g.guild.id;
    join(&app, &alice, &bob, &g).await;

    // New guilds get the two localized categories, each holding a channel.
    let names: Vec<&str> = g.categories.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Text", "Sprache"]);
    assert!(g.channels.iter().all(|c| c.category_id.is_some()));
    assert_eq!(g.guild.audio_bitrate, 128_000);

    // Members can't manage categories or channels.
    let path = format!("/guilds/{gid}/categories");
    assert_eq!(app.post(&path, &bob.token, json!({ "name": "x" })).await.0, StatusCode::FORBIDDEN);
    let cat: Value = app.post_ok(&path, &alice.token, json!({ "name": "Games" })).await;
    let cat_id = cat["id"].as_str().unwrap().to_owned();
    let ch: Value = app
        .post_ok(
            &format!("/guilds/{gid}/channels"),
            &alice.token,
            json!({ "name": "lol", "kind": "voice", "category_id": cat_id }),
        )
        .await;
    let ch_id = ch["id"].as_str().unwrap().to_owned();
    assert_eq!(ch["category_id"].as_str(), Some(cat_id.as_str()));

    // Renaming and moving.
    let (status, _) = app.patch(&format!("/channels/{ch_id}"), &alice.token, json!({ "name": "League" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        app.patch(&format!("/channels/{ch_id}"), &bob.token, json!({ "name": "x" })).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.patch(&format!("/categories/{cat_id}"), &alice.token, json!({ "name": "Spiele" })).await.0,
        StatusCode::NO_CONTENT
    );
    let g2 = guild(&app, &bob, gid).await;
    assert!(g2.categories.iter().any(|c| c.name == "Spiele"));
    assert!(g2.channels.iter().any(|c| c.name == "League"));

    // Deleting a category keeps its channels, uncategorized.
    assert_eq!(app.delete(&format!("/categories/{cat_id}"), &alice.token).await.0, StatusCode::NO_CONTENT);
    let g3 = guild(&app, &alice, gid).await;
    assert!(g3.channels.iter().any(|c| c.name == "League" && c.category_id.is_none()));

    // Deleting a channel.
    assert_eq!(app.delete(&format!("/channels/{ch_id}"), &bob.token).await.0, StatusCode::FORBIDDEN);
    assert_eq!(app.delete(&format!("/channels/{ch_id}"), &alice.token).await.0, StatusCode::NO_CONTENT);
    assert!(!guild(&app, &alice, gid).await.channels.iter().any(|c| c.name == "League"));

    // Bitrate: only listed values, only with MANAGE_GUILD.
    let gp = format!("/guilds/{gid}");
    assert_eq!(app.patch(&gp, &alice.token, json!({ "audio_bitrate": 100_000 })).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(app.patch(&gp, &bob.token, json!({ "audio_bitrate": 256_000 })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(app.patch(&gp, &alice.token, json!({ "audio_bitrate": 256_000 })).await.0, StatusCode::OK);
    assert_eq!(guild(&app, &bob, gid).await.guild.audio_bitrate, 256_000);

    // Only the owner can delete the guild.
    assert_eq!(app.delete(&gp, &bob.token).await.0, StatusCode::FORBIDDEN);
    assert_eq!(app.delete(&gp, &alice.token).await.0, StatusCode::NO_CONTENT);
    assert_eq!(app.get(&gp, &alice.token).await.0, StatusCode::NOT_FOUND);
    app.cleanup().await;
}

#[tokio::test]
async fn read_states_mutes_and_member_settings() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let g = create_guild(&app, &alice).await;
    let gid = g.guild.id;
    join(&app, &alice, &bob, &g).await;
    let text = text_channel(&g);
    let a_dev = device(&app, &alice).await;
    let b_dev = device(&app, &bob).await;

    send(&app, &alice, a_dev, text).await;
    let second = send(&app, &alice, a_dev, text).await;
    send(&app, &bob, b_dev, text).await; // own messages never count

    let unread = |states: &[ReadState]| states.iter().find(|s| s.channel_id == text).map(|s| s.unread);
    let states: Vec<ReadState> = app.get_ok("/users/@me/read-states", &bob.token).await;
    assert_eq!(unread(&states), Some(2));

    let ack = format!("/channels/{text}/read");
    assert_eq!(app.put(&ack, &bob.token, json!({ "message_id": second.id })).await.0, StatusCode::NO_CONTENT);
    let states: Vec<ReadState> = app.get_ok("/users/@me/read-states", &bob.token).await;
    assert_eq!(unread(&states), Some(0));
    send(&app, &alice, a_dev, text).await;
    let states: Vec<ReadState> = app.get_ok("/users/@me/read-states", &bob.token).await;
    assert_eq!(unread(&states), Some(1));

    // Mutes: until a time, or until unmuted; expired ones disappear.
    let mute = format!("/channels/{text}/mute");
    assert_eq!(app.put(&mute, &bob.token, json!({ "until": null })).await.0, StatusCode::NO_CONTENT);
    let mutes: Vec<ChannelMute> = app.get_ok("/users/@me/mutes", &bob.token).await;
    assert_eq!(mutes.len(), 1);
    assert!(mutes[0].until.is_none());
    let past = chrono::Utc::now() - chrono::Duration::minutes(1);
    assert_eq!(app.put(&mute, &bob.token, json!({ "until": past })).await.0, StatusCode::NO_CONTENT);
    let mutes: Vec<ChannelMute> = app.get_ok("/users/@me/mutes", &bob.token).await;
    assert!(mutes.is_empty());
    assert_eq!(app.delete(&mute, &bob.token).await.0, StatusCode::NO_CONTENT);

    // Name and notifications per guild.
    let me = format!("/guilds/{gid}/members/@me");
    assert_eq!(
        app.patch(&me, &bob.token, json!({ "nick": "Bobby", "notify_level": "mentions" })).await.0,
        StatusCode::NO_CONTENT
    );
    let g2: GuildWithChannels = guild(&app, &bob, gid).await;
    assert_eq!(g2.nick.as_deref(), Some("Bobby"));
    assert_eq!(g2.notify_level, NotifyLevel::Mentions);
    let members: Vec<GuildMember> = app.get_ok(&format!("/guilds/{gid}/members"), &alice.token).await;
    assert!(members.iter().any(|m| m.user.username == "bob" && m.nick.as_deref() == Some("Bobby")));
    assert_eq!(app.patch(&me, &bob.token, json!({ "nick": null })).await.0, StatusCode::NO_CONTENT);
    assert_eq!(guild(&app, &bob, gid).await.nick, None);
    app.cleanup().await;
}

#[tokio::test]
async fn profiles_and_status() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let carol = app.register("carol").await;
    let g = create_guild(&app, &alice).await;
    join(&app, &alice, &bob, &g).await;

    // Display name, description and status.
    let (status, _) = app
        .patch("/users/@me", &bob.token, json!({ "display_name": "Bob B.", "bio": "Hallo!", "status": "dnd" }))
        .await;
    assert_eq!(status, StatusCode::OK);
    let me: User = app.get_ok("/users/@me", &bob.token).await;
    assert_eq!(me.name(), "Bob B.");
    assert_eq!(me.username, "bob");
    let settings: MeSettings = app.get_ok("/users/@me/settings", &bob.token).await;
    assert_eq!((settings.status, settings.bio.as_str()), (UserStatus::Dnd, "Hallo!"));
    let too_long = "x".repeat(200);
    assert_eq!(app.patch("/users/@me", &bob.token, json!({ "bio": too_long })).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(app.patch("/users/@me", &bob.token, json!({ "display_name": null })).await.0, StatusCode::OK);
    let me: User = app.get_ok("/users/@me", &bob.token).await;
    assert_eq!(me.name(), "bob");

    // Avatar: images only, visible to others, replaced and removed.
    // The server stores images as they are; the content type is what counts.
    let png = b"PNG test bytes".to_vec();
    let (status, _) = app.send_bytes(Method::PUT, "/users/@me/avatar", &bob.token, "text/plain", b"x".to_vec()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, uploaded) =
        app.send_bytes(Method::PUT, "/users/@me/avatar", &bob.token, "image/png", png.clone()).await;
    assert_eq!(status, StatusCode::OK);
    let file = uploaded["id"].as_str().unwrap().to_owned();
    let profile: UserProfile = app.get_ok(&format!("/users/{}", bob.user.id), &alice.token).await;
    assert_eq!(profile.user.avatar_id.map(|f| f.to_string()), Some(file.clone()));
    assert_eq!(profile.bio, "Hallo!");
    let (status, bytes) = app.get_bytes(&format!("/files/{file}"), &carol.token).await;
    assert_eq!((status, bytes), (StatusCode::OK, png.clone()));
    let (status, _) = app.send_bytes(Method::PUT, "/users/@me/banner", &bob.token, "image/png", png).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(app.delete("/users/@me/avatar", &bob.token).await.0, StatusCode::NO_CONTENT);
    assert_eq!(app.get_bytes(&format!("/files/{file}"), &bob.token).await.0, StatusCode::NOT_FOUND);
    let profile: UserProfile = app.get_ok(&format!("/users/{}", bob.user.id), &alice.token).await;
    assert!(profile.user.avatar_id.is_none() && profile.banner_id.is_some());

    // Without a gateway connection nobody is online; your own status is always known.
    let presence: Vec<UserPresence> = app.get_ok("/presence", &alice.token).await;
    assert_eq!(presence, [UserPresence { user_id: alice.user.id, status: UserStatus::Online }]);
    assert_eq!(profile.status, None);
    app.cleanup().await;
}
