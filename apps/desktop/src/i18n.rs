//! UI texts in German and English. The language follows Windows unless the
//! user picks one in the settings.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::settings::Language;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    De,
    En,
}

impl Lang {
    pub fn code(self) -> &'static str {
        match self {
            Lang::De => "de",
            Lang::En => "en",
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_language(choice: Language) {
    let lang = match choice {
        Language::German => Lang::De,
        Language::English => Lang::En,
        Language::System => system_language(),
    };
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn current() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == Lang::En as u8 { Lang::En } else { Lang::De }
}

#[cfg(windows)]
fn system_language() -> Lang {
    use windows::Win32::Globalization::GetUserDefaultUILanguage;
    // SAFETY: no arguments, returns a LANGID.
    let langid = unsafe { GetUserDefaultUILanguage() };
    // The low 10 bits are the primary language; 0x07 is German.
    if langid & 0x3ff == 0x07 { Lang::De } else { Lang::En }
}

#[cfg(not(windows))]
fn system_language() -> Lang {
    match std::env::var("LANG") {
        Ok(lang) if lang.starts_with("de") => Lang::De,
        _ => Lang::En,
    }
}

/// The text for `key` in the current language (the key itself if unknown).
pub fn t(key: &'static str) -> &'static str {
    static TABLE: OnceLock<HashMap<&'static str, (&'static str, &'static str)>> = OnceLock::new();
    let table = TABLE.get_or_init(|| TEXTS.iter().map(|&(k, de, en)| (k, (de, en))).collect());
    match table.get(key) {
        Some(&(de, en)) => match current() {
            Lang::De => de,
            Lang::En => en,
        },
        None => {
            debug_assert!(false, "missing translation: {key}");
            key
        }
    }
}

/// (key, German, English)
const TEXTS: &[(&str, &str, &str)] = &[
    // Common
    ("common.cancel", "Abbrechen", "Cancel"),
    ("common.close", "Schließen", "Close"),
    ("common.done", "Fertig", "Done"),
    ("common.no", "Nein", "No"),
    ("common.off", "Aus", "Off"),
    ("common.unknown", "Unbekannt", "Unknown"),
    ("common.wait", "Einen Moment …", "One moment …"),
    ("common.yes", "Ja", "Yes"),
    // Login
    ("login.title", "Willkommen zurück", "Welcome back"),
    ("login.subtitle", "Melde dich bei deiner Sivryn-Instanz an.", "Sign in to your Sivryn instance."),
    ("login.register_title", "Konto erstellen", "Create an account"),
    ("login.register_subtitle", "Registriere dich auf einer Sivryn-Instanz.", "Register on a Sivryn instance."),
    ("login.login", "Anmelden", "Sign in"),
    ("login.register", "Registrieren", "Register"),
    ("login.no_account", "Noch kein Konto? Registrieren", "New here? Register"),
    ("login.have_account", "Schon ein Konto? Anmelden", "Already have an account? Sign in"),
    ("login.instance", "Instanz", "Instance"),
    ("login.username", "Benutzername", "Username"),
    ("login.password", "Passwort", "Password"),
    ("login.invite", "Einladungscode", "Invite code"),
    (
        "login.invite_hint",
        "Nur nötig, wenn die Instanz nur mit Einladung offen ist",
        "Only needed if the instance is invite-only",
    ),
    ("login.show_password", "Passwort anzeigen", "Show password"),
    ("login.hide_password", "Passwort verbergen", "Hide password"),
    (
        "login.footer",
        "Ende-zu-Ende-verschlüsselt mit MLS · Open Source (AGPL-3.0)",
        "End-to-end encrypted with MLS · Open source (AGPL-3.0)",
    ),
    // Toasts
    (
        "toast.session_ended",
        "Deine Sitzung wurde beendet. Bitte melde dich erneut an.",
        "Your session ended. Please sign in again.",
    ),
    (
        "toast.signed_out",
        "Deine Anmeldung ist abgelaufen. Bitte melde dich erneut an.",
        "Your sign-in expired. Please sign in again.",
    ),
    ("toast.restore_failed", "Automatische Anmeldung fehlgeschlagen", "Automatic sign-in failed"),
    // Shell
    ("shell.offline", "Keine Verbindung zum Server, neuer Versuch läuft …", "No connection to the server, retrying …"),
    ("shell.online", "Online", "Online"),
    ("shell.connecting", "Verbinde …", "Connecting …"),
    ("shell.offline_short", "Offline", "Offline"),
    ("shell.pick_channel", "Wähle einen Kanal", "Pick a channel"),
    (
        "shell.pick_channel_hint",
        "Textkanäle zum Schreiben, Sprachkanäle zum Reden und Bildschirmteilen.",
        "Text channels to write, voice channels to talk and share your screen.",
    ),
    ("shell.no_servers", "Noch keine Server", "No servers yet"),
    (
        "shell.no_servers_hint",
        "Erstelle einen Server für deine Freunde oder tritt mit einem Einladungscode bei.",
        "Create a server for your friends or join one with an invite code.",
    ),
    ("shell.create_server", "Server erstellen", "Create a server"),
    ("shell.join_server", "Server beitreten", "Join a server"),
    ("shell.invite", "Leute einladen", "Invite people"),
    ("shell.text_channels", "Textkanäle", "Text channels"),
    ("shell.voice_channels", "Sprachkanäle", "Voice channels"),
    // Dialogs
    ("create.title", "Server erstellen", "Create a server"),
    (
        "create.hint",
        "Ein Server mit Text- und Sprachkanal. Einladen kannst du danach.",
        "A server with a text and a voice channel. You can invite people afterwards.",
    ),
    ("create.placeholder", "Servername", "Server name"),
    ("create.action", "Erstellen", "Create"),
    ("join.title", "Server beitreten", "Join a server"),
    ("join.hint", "Gib den Einladungscode ein, den du bekommen hast.", "Enter the invite code you received."),
    ("join.placeholder", "Einladungscode", "Invite code"),
    ("join.action", "Beitreten", "Join"),
    ("invite.title", "Freunde einladen", "Invite friends"),
    (
        "invite.hint",
        "Schicke diesen Code an die Person, die du einladen möchtest.",
        "Send this code to the person you want to invite.",
    ),
    // Chat
    ("chat.message_to", "Nachricht an", "Message"),
    ("chat.e2ee", "E2EE", "E2EE"),
    ("chat.e2ee_hint", "Nachrichten sind Ende-zu-Ende-verschlüsselt (MLS)", "Messages are end-to-end encrypted (MLS)"),
    ("chat.welcome", "Willkommen in", "Welcome to"),
    (
        "chat.welcome_hint",
        "Das ist der Anfang des Kanals. Nachrichten sind Ende-zu-Ende-verschlüsselt.",
        "This is the start of the channel. Messages are end-to-end encrypted.",
    ),
    ("chat.locked", "Verschlüsselt, bevor dieses Gerät beigetreten ist", "Encrypted before this device joined"),
    (
        "chat.waiting_for_key",
        "Warte, bis ein Mitglied dieses Gerät zum verschlüsselten Kanal hinzufügt …",
        "Waiting for a member to add this device to the encrypted channel …",
    ),
    ("chat.today", "Heute", "Today"),
    ("chat.yesterday", "Gestern", "Yesterday"),
    // Calls
    ("call.join", "Beitreten", "Join voice"),
    ("call.connecting", "Verbinde …", "Connecting …"),
    ("call.connected", "Sprache verbunden", "Voice connected"),
    ("call.disconnected", "Die Verbindung zum Sprachkanal wurde getrennt.", "Disconnected from the voice channel."),
    ("call.nobody_here", "Noch ist niemand hier.", "Nobody is here yet."),
    ("call.people_here", "im Kanal", "in the channel"),
    ("call.in_call", "im Call", "in the call"),
    ("call.e2ee", "E2EE", "E2EE"),
    (
        "call.quality_hint",
        "Bildschirmübertragung bis 1440p60 mit AV1, wenn deine GPU es kann.",
        "Screen sharing up to 1440p60 with AV1 when your GPU supports it.",
    ),
    ("call.mute", "Stummschalten", "Mute"),
    ("call.unmute", "Stummschaltung aufheben", "Unmute"),
    ("call.deafen", "Ton aus", "Deafen"),
    ("call.undeafen", "Ton an", "Undeafen"),
    ("call.camera_on", "Kamera einschalten", "Turn on camera"),
    ("call.camera_off", "Kamera ausschalten", "Turn off camera"),
    ("call.camera_failed", "Kamera konnte nicht gestartet werden", "Could not start the camera"),
    ("call.share", "Bildschirm teilen", "Share screen"),
    ("call.stop_share", "Übertragung beenden", "Stop sharing"),
    ("call.leave", "Verlassen", "Leave"),
    ("call.mic_unavailable", "Mikrofon nicht verfügbar", "Microphone unavailable"),
    (
        "call.waiting_for_key",
        "Warte, bis ein Mitglied dieses Gerät zum verschlüsselten Call hinzufügt",
        "Waiting for a member to add this device to the encrypted call",
    ),
    ("call.user_volume", "Lautstärke", "Volume"),
    ("call.stream_volume", "Stream-Lautstärke", "Stream volume"),
    // Screen share
    ("share.title", "Bildschirm teilen", "Share your screen"),
    ("share.optimize", "Optimieren für", "Optimize for"),
    ("share.detail", "Scharfer Text", "Sharp text"),
    ("share.motion", "Flüssige Bewegung", "Smooth motion"),
    ("share.quality", "Qualität", "Quality"),
    ("share.audio", "Ton mitübertragen", "Share audio"),
    ("share.audio_failed", "Ton konnte nicht übertragen werden", "Could not share audio"),
    ("share.screens", "Bildschirme", "Screens"),
    ("share.windows", "Fenster", "Windows"),
    ("share.starting", "Übertragung startet", "Starting"),
    // Settings
    ("settings.title", "Einstellungen", "Settings"),
    ("settings.account", "Mein Konto", "My account"),
    ("settings.voice", "Sprache & Audio", "Voice & audio"),
    ("settings.video", "Video", "Video"),
    ("settings.keybinds", "Tastenkürzel", "Keybinds"),
    ("settings.appearance", "Darstellung", "Appearance"),
    ("settings.notifications", "Benachrichtigungen", "Notifications"),
    ("settings.about", "Über Sivryn", "About Sivryn"),
    ("settings.instance", "Instanz", "Instance"),
    ("settings.device", "Geräte-ID", "Device ID"),
    (
        "settings.device_hint",
        "Schlüssel und Nachrichtenverlauf sind verschlüsselt auf diesem Gerät gespeichert, der Schlüssel liegt in der Windows-Anmeldeinformationsverwaltung.",
        "Keys and message history are stored encrypted on this device; the key is kept in Windows Credential Manager.",
    ),
    ("settings.logout", "Abmelden", "Sign out"),
    ("settings.system_default", "Systemstandard", "System default"),
    // Voice & audio
    ("voice.input_device", "Eingabegerät", "Input device"),
    ("voice.output_device", "Ausgabegerät", "Output device"),
    ("voice.input_volume", "Eingangslautstärke", "Input volume"),
    ("voice.output_volume", "Ausgangslautstärke", "Output volume"),
    ("voice.mic_test", "Mikrofontest", "Mic test"),
    (
        "voice.mic_test_hint",
        "Du hörst dich selbst mit allen Filtern, so wie die anderen dich hören.",
        "You hear yourself with all filters applied, the way others hear you.",
    ),
    ("voice.start_test", "Test starten", "Start test"),
    ("voice.stop_test", "Test beenden", "Stop test"),
    ("voice.noise_suppression", "Rauschunterdrückung", "Noise suppression"),
    (
        "voice.ns_deepfilter_hint",
        "KI-Filter mit der besten Qualität: entfernt Tastatur, Lüfter und Hintergrundgeräusche. Etwa 30 ms zusätzliche Latenz.",
        "AI filter with the best quality: removes keyboards, fans and background noise. About 30 ms extra latency.",
    ),
    (
        "voice.ns_rnnoise_hint",
        "Leichter KI-Filter mit sehr wenig CPU-Last und 10 ms Latenz.",
        "Lightweight AI filter with very low CPU use and 10 ms latency.",
    ),
    (
        "voice.ns_webrtc_hint",
        "Klassischer Filter von WebRTC, gut gegen gleichmäßiges Rauschen.",
        "WebRTC's classic filter, good against steady noise.",
    ),
    (
        "voice.ns_off_hint",
        "Kein Filter: für gute Mikrofone in ruhiger Umgebung.",
        "No filter: for good microphones in quiet rooms.",
    ),
    ("voice.echo", "Echounterdrückung", "Echo cancellation"),
    (
        "voice.echo_hint",
        "Entfernt den Ton deiner Lautsprecher aus dem Mikrofon. Mit Kopfhörern nicht nötig.",
        "Removes your speakers' sound from the microphone. Not needed with headphones.",
    ),
    ("voice.agc", "Automatische Lautstärke", "Automatic gain"),
    ("voice.agc_hint", "Gleicht leise und laute Stimmen an.", "Evens out quiet and loud voices."),
    ("voice.mode", "Eingabemodus", "Input mode"),
    ("voice.mode_vad", "Sprachaktivierung", "Voice activity"),
    ("voice.mode_ptt", "Push-to-Talk", "Push to talk"),
    ("voice.auto_sensitivity", "Empfindlichkeit automatisch", "Automatic sensitivity"),
    (
        "voice.auto_sensitivity_hint",
        "Ein Spracherkenner entscheidet, wann du sprichst.",
        "A speech detector decides when you're talking.",
    ),
    ("voice.sensitivity", "Empfindlichkeit", "Sensitivity"),
    (
        "voice.sensitivity_hint",
        "Grün zeigt deinen aktuellen Pegel. Über der Schwelle wird übertragen.",
        "Green shows your current level. Above the threshold you're transmitted.",
    ),
    ("voice.ptt_release", "Loslass-Verzögerung", "Release delay"),
    // Video
    ("video.camera", "Kamera", "Camera"),
    ("video.camera_device", "Kamera", "Camera"),
    ("video.camera_quality", "Auflösung", "Resolution"),
    ("video.no_camera", "Keine Kamera gefunden", "No camera found"),
    (
        "video.no_mf",
        "Windows Media Foundation fehlt (Windows N ohne Media Feature Pack). Kamera und Hardware-Encoding sind deaktiviert, Bildschirmübertragung läuft per Software.",
        "Windows Media Foundation is missing (Windows N without the Media Feature Pack). Camera and hardware encoding are disabled; screen sharing uses software encoding.",
    ),
    ("video.screen", "Bildschirmübertragung", "Screen sharing"),
    (
        "video.screen_quality_hint",
        "Der Server kann die Qualität weiter begrenzen.",
        "The server may limit the quality further.",
    ),
    (
        "video.screen_audio_hint",
        "Fenster: nur der Ton dieser App. Bildschirm: der ganze Systemton ohne Sivryn, also ohne Echo der Stimmen.",
        "Window: only that app's sound. Screen: all system sound except Sivryn, so voices don't echo.",
    ),
    // Keybinds
    (
        "keys.hint",
        "Tastenkürzel funktionieren überall, auch wenn ein Spiel im Vordergrund ist. Maustasten 4 und 5 gehen auch.",
        "Keybinds work everywhere, even while a game has focus. Mouse buttons 4 and 5 work too.",
    ),
    ("keys.ptt", "Push-to-Talk", "Push to talk"),
    ("keys.ptt_hint", "Setzt den Eingabemodus auf Push-to-Talk.", "Switches the input mode to push to talk."),
    ("keys.mute", "Stummschalten umschalten", "Toggle mute"),
    ("keys.deafen", "Ton aus umschalten", "Toggle deafen"),
    ("keys.press_key", "Taste drücken … (Esc: entfernen)", "Press a key … (Esc: clear)"),
    ("keys.not_set", "Nicht festgelegt", "Not set"),
    ("keys.clear", "Entfernen", "Clear"),
    // Appearance
    ("appearance.theme", "Design", "Theme"),
    ("appearance.dark", "Dunkel", "Dark"),
    ("appearance.light", "Hell", "Light"),
    ("appearance.system", "System", "System"),
    ("appearance.language", "Sprache", "Language"),
    ("appearance.font_size", "Schriftgröße", "Font size"),
    ("appearance.compact", "Kompakte Nachrichten", "Compact messages"),
    (
        "appearance.compact_hint",
        "Mehr Nachrichten auf einen Blick, ohne Avatare.",
        "More messages at a glance, without avatars.",
    ),
    // Notifications
    ("notify.sounds", "Sounds", "Sounds"),
    (
        "notify.sounds_hint",
        "Töne beim Beitreten, Verlassen und Stummschalten.",
        "Sounds for joining, leaving and muting.",
    ),
    ("notify.messages", "Nachrichtenton", "Message sound"),
    (
        "notify.messages_hint",
        "Ein kurzer Ton bei neuen Nachrichten in anderen Kanälen.",
        "A short sound for new messages in other channels.",
    ),
    // About
    ("about.media", "Medien", "Media"),
    ("about.links", "Quellcode, Fehler melden und Selbst-Hosting:", "Source code, bug reports and self-hosting:"),
    ("common.remove", "Entfernen", "Remove"),
    ("common.save", "Speichern", "Save"),
    // Calls (DMs)
    ("call.start", "Anruf starten", "Start call"),
    ("call.start_video", "Videoanruf starten", "Start video call"),
    ("call.accept", "Annehmen", "Accept"),
    ("call.accept_video", "Mit Video annehmen", "Accept with video"),
    ("call.decline", "Ablehnen", "Decline"),
    ("call.incoming", "ruft dich an …", "is calling you …"),
    ("call.incoming_group", "Anruf in", "Call in"),
    ("call.ongoing", "Laufender Anruf", "Call in progress"),
    ("call.moved_afk", "Wegen Inaktivität in den AFK-Kanal verschoben.", "Moved to the AFK channel for inactivity."),
    // Chat
    ("chat.attach", "Dateien anhängen", "Attach files"),
    ("chat.delete_title", "Nachricht löschen", "Delete message"),
    (
        "chat.delete_hint",
        "Möchtest du diese Nachricht wirklich löschen? Das kann nicht rückgängig gemacht werden.",
        "Do you really want to delete this message? This cannot be undone.",
    ),
    (
        "chat.dm_start",
        "Das ist der Anfang deiner Direktnachrichten mit",
        "This is the beginning of your direct messages with",
    ),
    ("chat.group_start", "Das ist der Anfang dieser Gruppe.", "This is the beginning of this group."),
    ("chat.drop_here", "Dateien hier ablegen", "Drop files here"),
    ("chat.drop_hint", "Sie werden Ende-zu-Ende-verschlüsselt gesendet.", "They are sent end-to-end encrypted."),
    ("chat.drop_nowhere", "Öffne zuerst einen Chat, um Dateien zu senden.", "Open a chat first to send files."),
    ("chat.edited", "bearbeitet", "edited"),
    ("chat.editing", "Nachricht bearbeiten", "Editing message"),
    ("chat.editing_hint", "Esc zum Abbrechen, Enter zum Speichern", "Esc to cancel, Enter to save"),
    ("chat.reply_missing", "Ursprüngliche Nachricht nicht verfügbar", "Original message unavailable"),
    ("chat.replying_to", "Antwort an", "Replying to"),
    ("chat.uploading", "Wird verschlüsselt und hochgeladen …", "Encrypting and uploading …"),
    // DMs and groups
    ("dm.add", "Hinzufügen", "Add"),
    ("dm.add_people", "Personen hinzufügen", "Add people"),
    ("dm.leave", "Gruppe verlassen", "Leave group"),
    ("dm.no_friends", "Keine passenden Freunde.", "No matching friends."),
    ("dm.pick_hint", "Wähle Freunde aus. Mehrere ergeben eine Gruppe", "Pick friends. Several make a group"),
    ("dm.search_friends", "Freunde suchen", "Search friends"),
    // Emoji
    ("emoji.none", "Keine Emojis gefunden.", "No emoji found."),
    ("emoji.search", "Emoji suchen (Englisch)", "Search emoji"),
    ("emoji.title", "Emoji einfügen", "Insert emoji"),
    // Friends
    ("friends.title", "Freunde", "Friends"),
    ("friends.online", "Online", "Online"),
    ("friends.all", "Alle", "All"),
    ("friends.pending", "Ausstehend", "Pending"),
    ("friends.blocked", "Blockiert", "Blocked"),
    ("friends.add", "Freund hinzufügen", "Add friend"),
    ("friends.add_title", "Freund hinzufügen", "Add friend"),
    (
        "friends.add_hint",
        "Du kannst Freunde über ihren Benutzernamen hinzufügen.",
        "You can add friends by their username.",
    ),
    ("friends.add_placeholder", "Benutzername", "Username"),
    ("friends.send_request", "Freundschaftsanfrage senden", "Send friend request"),
    ("friends.request_sent", "Freundschaftsanfrage gesendet.", "Friend request sent."),
    ("friends.request_sent_to", "Anfrage gesendet an", "Request sent to"),
    ("friends.accept", "Annehmen", "Accept"),
    ("friends.decline", "Ablehnen", "Decline"),
    ("friends.cancel_request", "Anfrage zurückziehen", "Cancel request"),
    ("friends.incoming", "Eingehend", "Incoming"),
    ("friends.outgoing", "Ausgehend", "Outgoing"),
    ("friends.incoming_hint", "Eingehende Freundschaftsanfrage", "Incoming friend request"),
    ("friends.outgoing_hint", "Ausgehende Freundschaftsanfrage", "Outgoing friend request"),
    ("friends.blocked_hint", "Blockiert", "Blocked"),
    (
        "friends.none",
        "Noch keine Freunde. Füge welche über ihren Benutzernamen hinzu.",
        "No friends yet. Add some by their username.",
    ),
    ("friends.none_online", "Gerade ist niemand online.", "Nobody is online right now."),
    ("friends.none_pending", "Keine offenen Anfragen.", "No pending requests."),
    // Server settings
    ("guild.settings", "Server-Einstellungen", "Server settings"),
    ("guild.leave", "Server verlassen", "Leave server"),
    ("guild.overview", "Übersicht", "Overview"),
    ("guild.name", "Servername", "Server name"),
    ("guild.change_icon", "Icon ändern", "Change icon"),
    ("guild.pick_icon", "Server-Icon auswählen", "Choose server icon"),
    ("guild.afk_channel", "AFK-Kanal", "AFK channel"),
    (
        "guild.afk_channel_hint",
        "Wer so lange weder spricht noch Maus oder Tastatur benutzt, wird hierher verschoben.",
        "Members who neither speak nor use mouse or keyboard for this long are moved here.",
    ),
    ("guild.afk_none", "Kein AFK-Kanal", "No AFK channel"),
    ("guild.afk_timeout", "AFK-Zeitlimit", "AFK timeout"),
    ("guild.minutes", "Minuten", "minutes"),
    ("guild.minute", "Minute", "minute"),
    ("moderation.kick_title", "Mitglied kicken", "Kick member"),
    (
        "moderation.kick_hint",
        "Die Person kann mit einer neuen Einladung wieder beitreten.",
        "They can rejoin with a new invite.",
    ),
    ("moderation.ban_title", "Mitglied bannen", "Ban member"),
    (
        "moderation.ban_hint",
        "Die Person kann dem Server nicht wieder beitreten, bis du den Bann aufhebst.",
        "They can't rejoin until you lift the ban.",
    ),
    ("moderation.reason", "Grund (optional)", "Reason (optional)"),
    ("guild.bans", "Bans", "Bans"),
    (
        "guild.bans_hint",
        "Gebannte Nutzer können dem Server nicht wieder beitreten.",
        "Banned users cannot rejoin the server.",
    ),
    ("guild.no_bans", "Niemand ist gebannt.", "Nobody is banned."),
    ("guild.unban", "Entbannen", "Unban"),
    // Home
    ("home.title", "Direktnachrichten", "Direct messages"),
    ("home.direct_messages", "Direktnachrichten", "Direct messages"),
    ("home.new_dm", "Neue Nachricht", "New message"),
    ("home.create_dm", "Chat erstellen", "Create chat"),
    ("home.close_dm", "Schließen", "Close"),
    ("home.no_dms", "Noch keine Unterhaltungen.", "No conversations yet."),
    ("home.just_you", "Nur du", "Just you"),
    ("home.members", "Mitglieder", "members"),
    // Media
    ("media.download", "Herunterladen", "Download"),
    ("media.open_original", "Original öffnen", "Open original"),
    ("media.saved", "Gespeichert.", "Saved."),
    ("media.unavailable", "Nicht verfügbar", "Unavailable"),
    // Members
    ("members.show", "Mitgliederliste anzeigen", "Show member list"),
    ("members.hide", "Mitgliederliste ausblenden", "Hide member list"),
    ("members.online", "Online", "Online"),
    ("members.offline", "Offline", "Offline"),
    ("members.owner", "Serverbesitzer", "Server owner"),
    // Context menus
    ("menu.add_reaction", "Reaktion hinzufügen", "Add reaction"),
    ("menu.reply", "Antworten", "Reply"),
    ("menu.edit", "Bearbeiten", "Edit"),
    ("menu.delete", "Löschen", "Delete"),
    ("menu.copy_text", "Text kopieren", "Copy text"),
    ("menu.copy_selection", "Auswahl kopieren", "Copy selection"),
    ("menu.copy_link", "Link kopieren", "Copy link"),
    ("menu.open_link", "Link öffnen", "Open link"),
    ("menu.profile", "Profil anzeigen", "View profile"),
    ("menu.message", "Nachricht", "Message"),
    ("menu.add_friend", "Freund hinzufügen", "Add friend"),
    ("menu.remove_friend", "Freund entfernen", "Remove friend"),
    ("menu.mute_user", "Stummschalten", "Mute"),
    ("menu.disconnect", "Verbindung trennen", "Disconnect"),
    ("menu.kick", "Vom Server kicken", "Kick from server"),
    ("menu.ban", "Vom Server bannen", "Ban from server"),
    ("menu.block", "Blockieren", "Block"),
    ("menu.unblock", "Blockierung aufheben", "Unblock"),
    ("menu.mark_read", "Als gelesen markieren", "Mark as read"),
    // Privacy
    ("settings.privacy", "Privatsphäre", "Privacy"),
    ("privacy.server_dms", "Direktnachrichten von Server-Mitgliedern", "Direct messages from server members"),
    (
        "privacy.server_dms_hint",
        "Aus: Nur Freunde können dir schreiben und dich anrufen.",
        "Off: only friends can message and call you.",
    ),
    ("privacy.link_previews", "Link-Vorschauen erzeugen", "Generate link previews"),
    (
        "privacy.link_previews_hint",
        "Beim Senden lädt Sivryn die verlinkte Seite, um eine Vorschau anzuhängen. Empfänger rufen den Link nie auf.",
        "When sending, Sivryn loads the linked page to attach a preview. Recipients never open the link.",
    ),
    ("privacy.blocked", "Blockierte Nutzer", "Blocked users"),
    ("privacy.blocked_none", "Du hast niemanden blockiert.", "You haven't blocked anyone."),
    // Profile
    ("profile.member_since", "Mitglied seit", "Member since"),
    ("profile.mutual_servers", "Gemeinsame Server", "Mutual servers"),
    ("toast.copied", "In die Zwischenablage kopiert.", "Copied to the clipboard."),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for (key, _, _) in TEXTS {
            assert!(seen.insert(key), "duplicate key {key}");
        }
    }

    #[test]
    fn every_key_used_in_the_ui_exists() {
        let table: std::collections::HashSet<&str> = TEXTS.iter().map(|(k, _, _)| *k).collect();
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/ui");
        for entry in std::fs::read_dir(dir).unwrap() {
            let source = std::fs::read_to_string(entry.unwrap().path()).unwrap().replace("\\\"", "\"");
            for (i, _) in source.match_indices("t(\"") {
                // Skip calls like `opt("…")` that merely end in `t(`.
                if source[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let rest = &source[i + 3..];
                let key = &rest[..rest.find('"').unwrap()];
                assert!(table.contains(key), "missing translation for {key}");
            }
        }
    }
}
