use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::Duration;

use discord_rich_presence::{activity, DiscordIpc, DiscordIpcClient};

pub const CLIENT_ID: &str = "1282639576053059585";
const LOGO_URL: &str = "https://hate.bio/launcher/logo.png";

pub enum PresenceMsg {
    Idle,
    Playing { name: String, since: i64 },
}

static TX: OnceLock<mpsc::Sender<PresenceMsg>> = OnceLock::new();

pub fn init() {
    if CLIENT_ID.is_empty() {
        return;
    }
    let (tx, rx) = mpsc::channel::<PresenceMsg>();
    if TX.set(tx).is_err() {
        return;
    }
    std::thread::spawn(move || {
        let mut client: Option<DiscordIpcClient> = None;
        let mut last: Option<PresenceMsg> = Some(PresenceMsg::Idle);
        loop {
            match rx.recv_timeout(Duration::from_secs(20)) {
                Ok(msg) => last = Some(msg),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if client.is_none() {
                let mut candidate = DiscordIpcClient::new(CLIENT_ID);
                if candidate.connect().is_ok() {
                    client = Some(candidate);
                }
            }
            if let (Some(c), Some(l)) = (client.as_mut(), last.as_ref()) {
                if c.set_activity(build(l)).is_err() {
                    client = None;
                }
            }
        }
    });
}

pub fn set(msg: PresenceMsg) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(msg);
    }
}

fn clip(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.chars().count() > 120 {
        trimmed.chars().take(120).collect()
    } else {
        trimmed.to_string()
    }
}

fn build(msg: &PresenceMsg) -> activity::Activity<'static> {
    let assets = activity::Assets::new()
        .large_image(LOGO_URL)
        .large_text("Hate.bio Launcher");
    let buttons = vec![activity::Button::new("Get the launcher", "https://hate.bio/launcher")];
    match msg {
        PresenceMsg::Playing { name, since } => activity::Activity::new()
            .details(clip(name))
            .state("via Hate.bio Launcher")
            .timestamps(activity::Timestamps::new().start(*since))
            .assets(assets)
            .buttons(buttons),
        PresenceMsg::Idle => activity::Activity::new()
            .details("In the launcher")
            .state("Browsing the library")
            .assets(assets)
            .buttons(buttons),
    }
}
