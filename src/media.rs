use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::{Connection, Proxy};

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const OBJECT: &str = "/org/mpris/MediaPlayer2";
const INTERFACE: &str = "org.mpris.MediaPlayer2.Player";

#[derive(Clone, Copy)]
pub enum Action {
    Previous,
    PlayPause,
    Next,
    ToggleMute,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Status {
    pub present: bool,
    pub playing: bool,
    pub muted: bool,
}

/// D-Bus and wpctl both live on a worker thread; the event loop only ever
/// drops a message in the channel and reads the cached status.
pub struct Media {
    sender: Sender<Action>,
    status: Arc<Mutex<Status>>,
    generation: Arc<AtomicU64>,
}

impl Media {
    pub fn start() -> Self {
        let (sender, receiver) = channel::<Action>();
        let status = Arc::new(Mutex::new(Status::default()));
        let generation = Arc::new(AtomicU64::new(0));

        let shared = status.clone();
        let counter = generation.clone();

        thread::spawn(move || {
            let connection = Connection::session().ok();

            loop {
                let acted = match receiver.recv_timeout(Duration::from_secs(2)) {
                    Ok(action) => {
                        perform(connection.as_ref(), action);
                        true
                    }
                    Err(RecvTimeoutError::Timeout) => false,
                    Err(RecvTimeoutError::Disconnected) => return,
                };

                if acted {
                    // Players need a moment before PlaybackStatus reflects the call.
                    thread::sleep(Duration::from_millis(150));
                }

                let fresh = poll(connection.as_ref());
                let mut guard = shared.lock().unwrap();
                if *guard != fresh {
                    *guard = fresh;
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            }
        });

        Media { sender, status, generation }
    }

    pub fn send(&self, action: Action) {
        let _ = self.sender.send(action);
    }

    pub fn status(&self) -> Status {
        *self.status.lock().unwrap()
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
}

fn player(connection: &Connection) -> Option<Proxy<'_>> {
    let names = DBusProxy::new(connection).ok()?.list_names().ok()?;
    let chosen = choose(names.iter().map(|name| name.as_str()))?;

    Proxy::new(connection, chosen, OBJECT, INTERFACE).ok()
}

/// Pick an MPRIS player from the bus names: Spotify if present, otherwise
/// the first player listed.
fn choose<'a>(names: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut chosen: Option<String> = None;
    for name in names {
        if !name.starts_with(PREFIX) {
            continue;
        }
        if name.to_lowercase().contains("spotify") {
            return Some(name.to_string());
        }
        if chosen.is_none() {
            chosen = Some(name.to_string());
        }
    }
    chosen
}

fn perform(connection: Option<&Connection>, action: Action) {
    let method = match action {
        Action::Previous => "Previous",
        Action::PlayPause => "PlayPause",
        Action::Next => "Next",
        Action::ToggleMute => {
            let _ = std::process::Command::new("wpctl")
                .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
                .status();
            return;
        }
    };

    if let Some(proxy) = connection.and_then(player) {
        let _: zbus::Result<()> = proxy.call(method, &());
    }
}

fn poll(connection: Option<&Connection>) -> Status {
    let mut status = Status { muted: muted(), ..Default::default() };

    if let Some(proxy) = connection.and_then(player) {
        status.present = true;

        if let Ok(state) = proxy.get_property::<String>("PlaybackStatus") {
            status.playing = state == "Playing";
        }
    }

    status
}

fn muted() -> bool {
    std::process::Command::new("wpctl")
        .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
        .output()
        .map(|out| silenced(&String::from_utf8_lossy(&out.stdout)))
        .unwrap_or(false)
}

/// wpctl get-volume prints e.g. "Volume: 0.40 [MUTED]" when muted.
fn silenced(output: &str) -> bool {
    output.contains("MUTED")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choose_prefers_spotify() {
        let names = [
            "org.freedesktop.DBus",
            "org.mpris.MediaPlayer2.firefox.instance_1_42",
            "org.mpris.MediaPlayer2.spotify",
            "org.mpris.MediaPlayer2.vlc",
        ];
        assert_eq!(choose(names).as_deref(), Some("org.mpris.MediaPlayer2.spotify"));
    }

    #[test]
    fn choose_matches_spotify_case_insensitively() {
        let names = ["org.mpris.MediaPlayer2.vlc", "org.mpris.MediaPlayer2.Spotify.instance7"];
        assert_eq!(choose(names).as_deref(), Some("org.mpris.MediaPlayer2.Spotify.instance7"));
    }

    #[test]
    fn choose_falls_back_to_the_first_player() {
        let names = [":1.42", "org.mpris.MediaPlayer2.vlc", "org.mpris.MediaPlayer2.mpv"];
        assert_eq!(choose(names).as_deref(), Some("org.mpris.MediaPlayer2.vlc"));
    }

    #[test]
    fn choose_ignores_non_players() {
        assert_eq!(choose([]), None);
        assert_eq!(choose(["org.freedesktop.Notifications", "com.spotify.Client"]), None);
        assert_eq!(choose(["org.mpris.MediaPlayer2"]), None);
    }

    #[test]
    fn silenced_reads_wpctl_output() {
        assert!(silenced("Volume: 0.40 [MUTED]\n"));
        assert!(!silenced("Volume: 0.40\n"));
        assert!(!silenced(""));
    }
}
