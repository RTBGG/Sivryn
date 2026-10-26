//! Integration tests for the REST API and gateway against a real PostgreSQL.

mod common;

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use common::{TestApp, TestUser};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sivryn_client_core::{ApiClient, connect_gateway};
use sivryn_crypto::{Incoming, MlsDevice};
use sivryn_protocol::api::{
    ClaimedKeyPackage, GuildMember, GuildWithChannels, PendingWelcome, RegisterDeviceResponse, RegistrationMode,
    StoredCommit, VoiceJoinResponse,
};
use sivryn_protocol::gateway::{Event, ServerMessage};
use sivryn_protocol::model::{ChannelKind, Message, MessageContent, User};
use sivryn_protocol::{ChannelId, DeviceId};

fn text_channel(g: &GuildWithChannels) -> ChannelId {
    g.channels.iter().find(|c| c.kind == ChannelKind::Text).expect("text channel").id
}

fn voice_channel(g: &GuildWithChannels) -> ChannelId {
    g.channels.iter().find(|c| c.kind == ChannelKind::Voice).expect("voice channel").id
}

async fn create_guild(app: &TestApp, owner: &TestUser, name: &str) -> GuildWithChannels {
    app.post_ok("/guilds", &owner.token, json!({ "name": name })).await
}

/// `owner` invites `member` into `guild`.
async fn join_guild(app: &TestApp, owner: &TestUser, member: &TestUser, guild: &GuildWithChannels) {
    let invite: Value = app.post_ok(&format!("/guilds/{}/invites", guild.guild.id), &owner.token, json!(null)).await;
    let code = invite["code"].as_str().unwrap();
    let _: GuildWithChannels = app.post_ok(&format!("/invites/{code}"), &member.token, json!(null)).await;
}

async fn register_device(app: &TestApp, user: &TestUser, key_packages: Vec<String>) -> DeviceId {
    let resp: RegisterDeviceResponse =
        app.post_ok("/devices", &user.token, json!({ "name": "test", "key_packages": key_packages })).await;
    resp.device_id
}

#[tokio::test]
async fn first_user_is_admin_and_registration_is_invite_only() {
    let Some(app) = TestApp::spawn(RegistrationMode::InviteOnly).await else { return };

    let admin = app.register("admin").await;
    let (is_admin,): (bool,) = sqlx::query_as("SELECT is_admin FROM users WHERE id = $1")
        .bind(admin.user.id.0)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert!(is_admin, "first account must become admin");

    // Second account: rejected without or with a wrong code.
    assert_eq!(app.try_register("bob", None).await.0, StatusCode::FORBIDDEN);
    assert_eq!(app.try_register("bob", Some("nope")).await.0, StatusCode::FORBIDDEN);

    // Only admins can create instance invites.
    let invite: Value = app.post_ok("/instance/invites", &admin.token, json!(null)).await;
    let code = invite["code"].as_str().unwrap().to_owned();
    let (status, json) = app.try_register("bob", Some(&code)).await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let bob: User = serde_json::from_value(json["user"].clone()).unwrap();

    // Instance invites are single use.
    assert_eq!(app.try_register("carol", Some(&code)).await.0, StatusCode::FORBIDDEN);

    let body = json!({ "username": "bob", "password": common::PASSWORD });
    let (status, json) = app.request(reqwest::Method::POST, "/auth/login", None, Some(&body)).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    assert_eq!(json["user"]["id"], bob.id.to_string());

    // Clear reasons: no code at all versus a wrong one.
    assert_eq!(app.try_register("dave", None).await.1["error"], "invite_required");
    assert_eq!(app.try_register("dave", Some("nope")).await.1["error"], "invalid_invite");

    // A server invite lets someone register too, and they join that server.
    let guild = create_guild(&app, &admin, "Lobby").await;
    let invite: Value = app.post_ok(&format!("/guilds/{}/invites", guild.guild.id), &admin.token, json!(null)).await;
    let guild_code = invite["code"].as_str().unwrap().to_owned();
    let (status, json) = app.try_register("erin", Some(&format!(" {guild_code} "))).await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    let erin: User = serde_json::from_value(json["user"].clone()).unwrap();
    let members: Vec<GuildMember> = app.get_ok(&format!("/guilds/{}/members", guild.guild.id), &admin.token).await;
    assert!(members.iter().any(|m| m.user.id == erin.id), "erin joined the server");

    app.cleanup().await;
}

#[tokio::test]
async fn usernames_are_unique_case_insensitively() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    app.register("Alice").await;
    let (status, _) = app.try_register("alice", None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    app.cleanup().await;
}

#[tokio::test]
async fn login_logout_and_session_checks() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;

    let login = |password: &'static str| {
        let body = json!({ "username": "ALICE", "password": password });
        let app = &app;
        async move { app.request(reqwest::Method::POST, "/auth/login", None, Some(&body)).await }
    };
    assert_eq!(login("wrong password").await.0, StatusCode::UNAUTHORIZED);
    let (status, json) = login(common::PASSWORD).await;
    assert_eq!(status, StatusCode::OK, "{json}");
    let token = json["token"].as_str().unwrap().to_owned();

    let me: User = app.get_ok("/users/@me", &token).await;
    assert_eq!(me.id, alice.user.id);

    let (status, _) = app.post("/auth/logout", &token, json!(null)).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(app.get("/users/@me", &token).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(app.get("/users/@me", "garbage").await.0, StatusCode::UNAUTHORIZED);

    // The other session is unaffected.
    assert_eq!(app.get("/users/@me", &alice.token).await.0, StatusCode::OK);
    app.cleanup().await;
}

#[tokio::test]
async fn guild_lifecycle_and_permissions() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;

    let guild = create_guild(&app, &alice, "  Friends  ").await;
    assert_eq!(guild.guild.name, "Friends");
    assert_eq!(guild.channels.len(), 2);
    let (everyone,): (i64,) = sqlx::query_as("SELECT count(*) FROM roles WHERE guild_id = $1 AND is_everyone")
        .bind(guild.guild.id.0)
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(everyone, 1);

    // Non-members can't see the guild or its channels: 404, not 403.
    let gid = guild.guild.id;
    let text = text_channel(&guild);
    assert_eq!(app.get(&format!("/guilds/{gid}"), &bob.token).await.0, StatusCode::NOT_FOUND);
    assert_eq!(app.get(&format!("/channels/{text}/messages"), &bob.token).await.0, StatusCode::NOT_FOUND);
    assert_eq!(app.get(&format!("/guilds/{gid}/members"), &bob.token).await.0, StatusCode::NOT_FOUND);

    join_guild(&app, &alice, &bob, &guild).await;
    let members: Vec<GuildMember> = app.get_ok(&format!("/guilds/{gid}/members"), &bob.token).await;
    let names: Vec<&str> = members.iter().map(|m| m.user.username.as_str()).collect();
    assert_eq!(names, ["alice", "bob"]);

    let bobs_guilds: Vec<GuildWithChannels> = app.get_ok("/guilds", &bob.token).await;
    assert_eq!(bobs_guilds.len(), 1);

    // Regular members can't manage channels; the owner can.
    let body = json!({ "name": "stream", "kind": "voice", "e2ee": true });
    let (status, _) = app.post(&format!("/guilds/{gid}/channels"), &bob.token, body.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, json) = app.post(&format!("/guilds/{gid}/channels"), &alice.token, body).await;
    assert_eq!(status, StatusCode::CREATED, "{json}");
    assert_eq!(json["position"], 2);
    assert_eq!(json["e2ee"], true);

    // Accepting the same invite twice is harmless.
    join_guild(&app, &alice, &bob, &guild).await;
    let members: Vec<GuildMember> = app.get_ok(&format!("/guilds/{gid}/members"), &alice.token).await;
    assert_eq!(members.len(), 2);

    app.cleanup().await;
}

#[tokio::test]
async fn key_packages_are_claimed_once_per_device_and_only_by_guild_mates() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let mallory = app.register("mallory").await;

    let laptop = register_device(&app, &bob, vec!["kp-l1".into(), "kp-l2".into()]).await;
    let desktop = register_device(&app, &bob, vec!["kp-d1".into()]).await;

    let guild = create_guild(&app, &alice, "g").await;
    join_guild(&app, &alice, &bob, &guild).await;
    let claim_path = format!("/users/{}/key-packages/claim", bob.user.id);

    // Strangers can't drain Bob's key packages.
    assert_eq!(app.post(&claim_path, &mallory.token, json!(null)).await.0, StatusCode::NOT_FOUND);

    let mut first: Vec<ClaimedKeyPackage> = app.post_ok(&claim_path, &alice.token, json!(null)).await;
    first.sort_by_key(|c| c.key_package.clone());
    let got: Vec<(DeviceId, &str)> = first.iter().map(|c| (c.device_id, c.key_package.as_str())).collect();
    assert_eq!(got, [(desktop, "kp-d1"), (laptop, "kp-l1")]);

    // Each package is handed out once; the desktop has none left.
    let second: Vec<ClaimedKeyPackage> = app.post_ok(&claim_path, &alice.token, json!(null)).await;
    let got: Vec<(DeviceId, &str)> = second.iter().map(|c| (c.device_id, c.key_package.as_str())).collect();
    assert_eq!(got, [(laptop, "kp-l2")]);
    let third: Vec<ClaimedKeyPackage> = app.post_ok(&claim_path, &alice.token, json!(null)).await;
    assert!(third.is_empty());

    // Top-up only for one's own devices.
    let top_up = format!("/devices/{laptop}/key-packages");
    assert_eq!(app.post(&top_up, &alice.token, json!({ "key_packages": ["x"] })).await.0, StatusCode::FORBIDDEN);
    assert_eq!(app.post(&top_up, &bob.token, json!({ "key_packages": ["kp-l3"] })).await.0, StatusCode::NO_CONTENT);
    let fourth: Vec<ClaimedKeyPackage> = app.post_ok(&claim_path, &alice.token, json!(null)).await;
    assert_eq!(fourth.len(), 1);
    assert_eq!(fourth[0].key_package, "kp-l3");

    // Devices can see how many packages they have left, so they know when to top up.
    assert_eq!(app.get(&top_up, &alice.token).await.0, StatusCode::FORBIDDEN);
    app.post(&top_up, &bob.token, json!({ "key_packages": ["kp-l4", "kp-l5"] })).await;
    let left: Value = app.get_ok(&top_up, &bob.token).await;
    assert_eq!(left["count"], 2);

    app.cleanup().await;
}

#[tokio::test]
async fn commits_are_ordered_per_epoch_and_welcomes_delivered_once() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let guild = create_guild(&app, &alice, "g").await;
    join_guild(&app, &alice, &bob, &guild).await;
    let channel = text_channel(&guild);
    let a_dev = register_device(&app, &alice, vec![]).await;
    let b_dev = register_device(&app, &bob, vec![]).await;
    let commits = format!("/channels/{channel}/mls/commits");

    let commit = |device: DeviceId, epoch: u64, payload: &str, welcome: Option<Value>| json!({ "device_id": device, "epoch": epoch, "commit": payload, "welcome": welcome });
    let welcome = json!({ "recipients": [b_dev], "welcome": "W", "ratchet_tree": "T" });

    assert_eq!(app.post(&commits, &alice.token, commit(a_dev, 0, "c0", Some(welcome))).await.0, StatusCode::NO_CONTENT);
    // Losing the race for epoch 0.
    assert_eq!(app.post(&commits, &bob.token, commit(b_dev, 0, "other", None)).await.0, StatusCode::CONFLICT);
    // Can't commit with someone else's device.
    assert_eq!(app.post(&commits, &bob.token, commit(a_dev, 1, "c1", None)).await.0, StatusCode::FORBIDDEN);
    assert_eq!(app.post(&commits, &bob.token, commit(b_dev, 1, "c1", None)).await.0, StatusCode::NO_CONTENT);

    let all: Vec<StoredCommit> = app.get_ok(&format!("{commits}?since=0"), &bob.token).await;
    assert_eq!(all.iter().map(|c| (c.epoch, c.commit.as_str())).collect::<Vec<_>>(), [(0, "c0"), (1, "c1")]);
    let since1: Vec<StoredCommit> = app.get_ok(&format!("{commits}?since=1"), &bob.token).await;
    assert_eq!(since1.len(), 1);

    let welcomes = format!("/devices/{b_dev}/welcomes");
    assert_eq!(app.get(&welcomes, &alice.token).await.0, StatusCode::FORBIDDEN);
    let pending: Vec<PendingWelcome> = app.get_ok(&welcomes, &bob.token).await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].channel_id, channel);
    assert_eq!(pending[0].welcome.welcome, "W");
    assert_eq!(pending[0].welcome.ratchet_tree, "T");
    let again: Vec<PendingWelcome> = app.get_ok(&welcomes, &bob.token).await;
    assert!(again.is_empty());

    app.cleanup().await;
}

#[tokio::test]
async fn messages_validate_and_paginate() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let guild = create_guild(&app, &alice, "g").await;
    let text = text_channel(&guild);
    let voice = voice_channel(&guild);
    let a_dev = register_device(&app, &alice, vec![]).await;
    let b_dev = register_device(&app, &bob, vec![]).await;
    let path = format!("/channels/{text}/messages");

    let msg = |device: DeviceId, ct: &str| json!({ "device_id": device, "epoch": 3, "ciphertext": ct });
    assert_eq!(app.post(&path, &alice.token, msg(a_dev, "")).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(app.post(&path, &alice.token, msg(b_dev, "x")).await.0, StatusCode::FORBIDDEN);
    // Voice channels have a chat of their own.
    assert_eq!(
        app.post(&format!("/channels/{voice}/messages"), &alice.token, msg(a_dev, "x")).await.0,
        StatusCode::CREATED
    );
    assert_eq!(app.post(&path, &bob.token, msg(b_dev, "x")).await.0, StatusCode::NOT_FOUND);

    let mut sent = Vec::new();
    for i in 0..5 {
        let m: Message = app.post_ok(&path, &alice.token, msg(a_dev, &format!("ct{i}"))).await;
        assert_eq!(m.epoch, 3);
        assert_eq!(m.sender_device_id, a_dev);
        sent.push(m);
    }

    // Newest first, `before` pages backwards.
    let page: Value = app.get_ok(&format!("{path}?limit=2"), &alice.token).await;
    let cts: Vec<&str> =
        page["messages"].as_array().unwrap().iter().map(|m| m["ciphertext"].as_str().unwrap()).collect();
    assert_eq!(cts, ["ct4", "ct3"]);
    let page: Value = app.get_ok(&format!("{path}?limit=2&before={}", sent[3].id), &alice.token).await;
    let cts: Vec<&str> =
        page["messages"].as_array().unwrap().iter().map(|m| m["ciphertext"].as_str().unwrap()).collect();
    assert_eq!(cts, ["ct2", "ct1"]);

    // `after` walks forwards (oldest first) for catching up.
    let page: Value = app.get_ok(&format!("{path}?limit=2&after={}", sent[1].id), &alice.token).await;
    let cts: Vec<&str> =
        page["messages"].as_array().unwrap().iter().map(|m| m["ciphertext"].as_str().unwrap()).collect();
    assert_eq!(cts, ["ct2", "ct3"]);
    let page: Value = app.get_ok(&format!("{path}?after={}", sent[4].id), &alice.token).await;
    assert!(page["messages"].as_array().unwrap().is_empty());

    app.cleanup().await;
}

#[tokio::test]
async fn history_copies_are_kept_and_sharing_can_be_turned_off() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let guild = create_guild(&app, &alice, "g").await;
    assert!(guild.guild.share_history, "on by default");
    let text = text_channel(&guild);
    let a_dev = register_device(&app, &alice, vec![]).await;
    let path = format!("/channels/{text}/messages");

    // A message with its history copy, and a key handover (no target).
    let m: Message = app
        .post_ok(
            &path,
            &alice.token,
            json!({ "device_id": a_dev, "epoch": 1, "ciphertext": "ct", "history": "sealed" }),
        )
        .await;
    assert_eq!(m.history.as_deref(), Some("sealed"));
    let keys = json!({ "device_id": a_dev, "epoch": 1, "ciphertext": "keys", "kind": "history_keys" });
    let k: Message = app.post_ok(&path, &alice.token, keys).await;
    assert_eq!(k.kind, sivryn_protocol::model::MessageKind::HistoryKeys);
    let with_target =
        json!({ "device_id": a_dev, "epoch": 1, "ciphertext": "keys", "kind": "history_keys", "target_id": m.id });
    assert_eq!(app.post(&path, &alice.token, with_target).await.0, StatusCode::BAD_REQUEST);
    let page: Value = app.get_ok(&format!("{path}?limit=5"), &alice.token).await;
    assert_eq!(page["messages"][1]["history"], "sealed");

    let (status, updated) =
        app.patch(&format!("/guilds/{}", guild.guild.id), &alice.token, json!({ "share_history": false })).await;
    assert_eq!(status, StatusCode::OK, "{updated}");
    assert_eq!(updated["share_history"], false);

    app.cleanup().await;
}

#[tokio::test]
async fn voice_presence_needs_membership_and_survives_livekit_outage() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let mallory = app.register("mallory").await;
    let guild = create_guild(&app, &alice, "g").await;
    let path = format!("/guilds/{}/voice", guild.guild.id);

    assert_eq!(app.get(&path, &mallory.token).await.0, StatusCode::NOT_FOUND);
    // No LiveKit in tests: every voice channel is listed, just empty.
    let states: Value = app.get_ok(&path, &alice.token).await;
    let states = states.as_array().unwrap();
    assert_eq!(states.len(), 1);
    assert_eq!(states[0]["channel_id"], json!(voice_channel(&guild)));
    assert!(states[0]["participants"].as_array().unwrap().is_empty());

    app.cleanup().await;
}

#[tokio::test]
async fn voice_join_issues_scoped_livekit_token() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let guild = create_guild(&app, &alice, "g").await;
    let voice = voice_channel(&guild);

    assert_eq!(
        app.post(&format!("/channels/{}/voice/join", text_channel(&guild)), &alice.token, json!(null)).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        app.post(&format!("/channels/{voice}/voice/join"), &bob.token, json!(null)).await.0,
        StatusCode::NOT_FOUND
    );

    let join: VoiceJoinResponse =
        app.post_ok(&format!("/channels/{voice}/voice/join"), &alice.token, json!(null)).await;
    assert_eq!(join.livekit_url, "wss://livekit.test");
    assert_eq!(join.screen_share.max_height, 1440);
    assert!(!join.e2ee);

    let payload = join.token.split('.').nth(1).expect("JWT payload");
    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
    assert_eq!(claims["sub"], alice.user.id.to_string());
    assert_eq!(claims["name"], "alice");
    let video = &claims["video"];
    assert_eq!(video["room"], voice.to_string());
    assert_eq!(video["roomJoin"], true);
    let sources: Vec<&str> =
        video["canPublishSources"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
    for expected in ["microphone", "camera", "screen_share", "screen_share_audio"] {
        assert!(sources.contains(&expected), "missing {expected} in {sources:?}");
    }

    app.cleanup().await;
}

/// Full path: real MLS key packages through the delivery service, a welcome,
/// and an encrypted message that arrives over the gateway and decrypts.
#[tokio::test]
async fn end_to_end_encrypted_message_over_gateway() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let guild = create_guild(&app, &alice, "g").await;
    join_guild(&app, &alice, &bob, &guild).await;
    let channel = text_channel(&guild);

    let mut alice_mls = MlsDevice::new(&alice.user.id.to_string()).unwrap();
    let mut bob_mls = MlsDevice::new(&bob.user.id.to_string()).unwrap();
    let a_dev = register_device(&app, &alice, alice_mls.key_packages(2).unwrap()).await;
    let b_dev = register_device(&app, &bob, bob_mls.key_packages(2).unwrap()).await;

    // Bob listens on the gateway as his device.
    let bob_api = ApiClient::new(&app.url).unwrap().with_token(bob.token.clone());
    let mut gateway = connect_gateway(&bob_api, Some(b_dev)).await.unwrap();
    // Next server message, skipping heartbeat acks.
    let mut next = async || loop {
        match tokio::time::timeout(Duration::from_secs(5), gateway.events.recv()).await.unwrap().unwrap() {
            ServerMessage::HeartbeatAck { .. } => continue,
            msg => break msg,
        }
    };
    assert!(matches!(next().await, ServerMessage::Hello { .. }));
    match next().await {
        ServerMessage::Ready { user, guilds } => {
            assert_eq!(user.id, bob.user.id);
            assert_eq!(guilds.len(), 1);
        }
        other => panic!("expected ready, got {other:?}"),
    }

    // Alice creates the channel group and adds Bob's device.
    alice_mls.create_group(channel).unwrap();
    let claimed: Vec<ClaimedKeyPackage> =
        app.post_ok(&format!("/users/{}/key-packages/claim", bob.user.id), &alice.token, json!(null)).await;
    assert_eq!(claimed.len(), 1);
    let pending = alice_mls.add_members(channel, &[claimed[0].key_package.clone()]).unwrap();
    let body = json!({
        "device_id": a_dev,
        "epoch": pending.epoch,
        "commit": pending.commit,
        "welcome": { "recipients": [b_dev], "welcome": pending.welcome, "ratchet_tree": pending.ratchet_tree },
    });
    let (status, json) = app.post(&format!("/channels/{channel}/mls/commits"), &alice.token, body).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{json}");
    alice_mls.confirm_commit(channel).unwrap();

    // Bob sees the welcome addressed to his device (after the commit event).
    let mut welcomed = false;
    for _ in 0..4 {
        match next().await {
            ServerMessage::Event(Event::MlsWelcome { channel_id, welcome }) => {
                assert_eq!(channel_id, channel);
                assert_eq!(welcome.recipients, [b_dev]);
                bob_mls.join(channel, &welcome.welcome, &welcome.ratchet_tree).unwrap();
                welcomed = true;
                break;
            }
            ServerMessage::Event(Event::MlsCommit { .. } | Event::PresenceUpdate { .. }) => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(welcomed, "welcome not delivered over gateway");

    let content = MessageContent { body: "hallo bob".into(), ..Default::default() };
    let (epoch, ciphertext) = alice_mls.encrypt(channel, &content).unwrap();
    let (status, json) = app
        .post(
            &format!("/channels/{channel}/messages"),
            &alice.token,
            json!({ "device_id": a_dev, "epoch": epoch, "ciphertext": ciphertext }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");

    loop {
        match next().await {
            ServerMessage::Event(Event::MessageCreate { message, .. }) => {
                assert_eq!(message.author_id, alice.user.id);
                match bob_mls.process(channel, &message.ciphertext).unwrap() {
                    Incoming::Message(m) => assert_eq!(m.body, "hallo bob"),
                    other => panic!("expected message, got {other:?}"),
                }
                break;
            }
            ServerMessage::Event(_) => {}
            other => panic!("unexpected {other:?}"),
        }
    }

    // The server only ever stored ciphertext.
    let (stored,): (String,) = sqlx::query_as("SELECT ciphertext FROM messages").fetch_one(&app.db).await.unwrap();
    assert!(!stored.contains("hallo"));

    app.cleanup().await;
}

#[tokio::test]
async fn gateway_rejects_bad_tokens_and_foreign_devices() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let b_dev = register_device(&app, &bob, vec![]).await;

    let bad = ApiClient::new(&app.url).unwrap().with_token("nope".into());
    let mut conn = connect_gateway(&bad, None).await.unwrap();
    let recv = async |conn: &mut sivryn_client_core::GatewayConnection| {
        tokio::time::timeout(Duration::from_secs(5), conn.events.recv()).await.unwrap()
    };
    assert!(matches!(recv(&mut conn).await, Some(ServerMessage::Hello { .. })));
    let second = recv(&mut conn).await;
    assert!(matches!(second, Some(ServerMessage::InvalidSession { .. })), "got {second:?}");

    // Alice can't identify as Bob's device.
    let alice_api = ApiClient::new(&app.url).unwrap().with_token(alice.token.clone());
    let mut conn = connect_gateway(&alice_api, Some(b_dev)).await.unwrap();
    assert!(matches!(recv(&mut conn).await, Some(ServerMessage::Hello { .. })));
    let second = recv(&mut conn).await;
    assert!(matches!(second, Some(ServerMessage::InvalidSession { .. })), "got {second:?}");
    assert!(recv(&mut conn).await.is_none(), "connection should close");

    app.cleanup().await;
}

#[tokio::test]
async fn invite_links_show_a_page_and_a_preview() {
    let Some(app) = TestApp::spawn(RegistrationMode::Open).await else { return };
    let alice = app.register("alice").await;
    let guild = create_guild(&app, &alice, "Café <Crew>").await;
    let invite: Value = app.post_ok(&format!("/guilds/{}/invites", guild.guild.id), &alice.token, json!(null)).await;
    let code = invite["code"].as_str().unwrap().to_owned();

    // The preview needs no sign-in.
    let (status, preview) = app.request::<()>(reqwest::Method::GET, &format!("/invites/{code}"), None, None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["guild"]["name"], "Café <Crew>");
    assert_eq!(preview["guild"]["member_count"], 1);
    let (status, _) = app.request::<()>(reqwest::Method::GET, "/invites/nope", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let http = reqwest::Client::new();
    for path in [format!("/{code}"), format!("/invite/{code}")] {
        let resp = http.get(format!("{}{path}", app.url)).header("Accept-Language", "de-DE").send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let html = resp.text().await.unwrap();
        assert!(html.contains("Café &lt;Crew&gt;"), "{html}");
        assert!(html.contains(&format!("sivryn://join/{}/{code}", app.url.trim_start_matches("http://"))), "{html}");
        assert!(html.contains("In Sivryn öffnen"));
    }
    let resp = http.get(format!("{}/doesnotexist", app.url)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let resp = http.get(format!("{}/", app.url)).send().await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.text().await.unwrap().contains("Test"));
    assert_eq!(http.get(format!("{}/healthz", app.url)).send().await.unwrap().text().await.unwrap(), "ok");

    app.cleanup().await;
}
