use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::message::Type;
use zbus::MatchRule;

const PREFIX: &str = "org.mpris.MediaPlayer2.";
const OBJECT: &str = "/org/mpris/MediaPlayer2";
const INTERFACE: &str = "org.mpris.MediaPlayer2.Player";

/// How often to re-read everything once a watcher has stopped.
const POLL: Duration = Duration::from_secs(2);

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

/// What the worker is woken by.
enum Event {
    Act(Action),
    /// A player appeared, quit, or changed its properties.
    Player,
    /// A sink, or the choice of default sink, changed.
    Sound,
    /// A watcher stopped, so changes may go unnoticed.
    Lost,
}

/// D-Bus and wpctl both live on a worker thread; the event loop only ever
/// drops a message in the channel and reads the cached status. The worker
/// re-reads the player when D-Bus signals a change and the mute state when
/// PipeWire's pulse server does, and falls back to polling if either watcher
/// stops.
pub struct Media {
    sender: Sender<Event>,
    status: Arc<Mutex<Status>>,
    generation: Arc<AtomicU64>,
}

impl Media {
    pub fn start() -> Self {
        let (sender, receiver) = channel::<Event>();
        let status = Arc::new(Mutex::new(Status::default()));
        let generation = Arc::new(AtomicU64::new(0));

        let shared = status.clone();
        let counter = generation.clone();
        let events = sender.clone();

        thread::spawn(move || {
            let connection = Connection::session().ok();
            match &connection {
                Some(connection) => watch_players(connection.clone(), events.clone()),
                None => {
                    let _ = events.send(Event::Lost);
                }
            }
            watch_sound(events);

            let mut fresh = Status::default();
            let mut polling = false;
            let mut pending = Pending { player: true, sound: true, ..Default::default() };

            loop {
                if pending.lost {
                    polling = true;
                }
                for action in pending.actions.drain(..) {
                    perform(connection.as_ref(), action);
                    if polling {
                        // Players need a moment before PlaybackStatus reflects the call.
                        thread::sleep(Duration::from_millis(150));
                        pending.player = true;
                        pending.sound = true;
                    }
                }

                if pending.player {
                    (fresh.present, fresh.playing) = playback(connection.as_ref());
                }
                if pending.sound {
                    fresh.muted = muted();
                }

                let mut guard = shared.lock().unwrap();
                if *guard != fresh {
                    *guard = fresh;
                    counter.fetch_add(1, Ordering::Relaxed);
                }
                drop(guard);

                pending = match wait(&receiver, polling.then_some(POLL)) {
                    Some(pending) => pending,
                    None => return,
                };
            }
        });

        Media { sender, status, generation }
    }

    pub fn send(&self, action: Action) {
        let _ = self.sender.send(Event::Act(action));
    }

    pub fn status(&self) -> Status {
        *self.status.lock().unwrap()
    }

    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }
}

/// Work gathered from every event that has arrived, so a burst of signals
/// costs one re-read.
#[derive(Default)]
struct Pending {
    actions: Vec<Action>,
    player: bool,
    sound: bool,
    lost: bool,
}

impl Pending {
    fn add(&mut self, event: Event) {
        match event {
            Event::Act(action) => self.actions.push(action),
            Event::Player => self.player = true,
            Event::Sound => self.sound = true,
            Event::Lost => {
                self.lost = true;
                self.player = true;
                self.sound = true;
            }
        }
    }
}

/// Block for the next event, or until `poll` passes, then take whatever else
/// is already queued. None once the Media has been dropped.
fn wait(receiver: &Receiver<Event>, poll: Option<Duration>) -> Option<Pending> {
    let mut pending = Pending::default();
    let first = if let Some(poll) = poll {
        match receiver.recv_timeout(poll) {
            Ok(event) => Some(event),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    } else {
        Some(receiver.recv().ok()?)
    };

    match first {
        Some(event) => pending.add(event),
        None => {
            pending.player = true;
            pending.sound = true;
        }
    }
    for event in receiver.try_iter() {
        pending.add(event);
    }
    Some(pending)
}

/// Signal Event::Player whenever an MPRIS player comes, goes, or changes.
fn watch_players(connection: Connection, events: Sender<Event>) {
    thread::spawn(move || {
        let streams = rules().and_then(|(changed, owners)| {
            Ok((
                MessageIterator::for_match_rule(changed, &connection, None)?,
                MessageIterator::for_match_rule(owners, &connection, None)?,
            ))
        });
        let Ok((changed, owners)) = streams else {
            let _ = events.send(Event::Lost);
            return;
        };

        // Each iterator blocks, so the second gets a thread of its own.
        let also = events.clone();
        thread::spawn(move || relay(owners, also));
        relay(changed, events);
    });
}

/// Signals for a player's properties changing, and for a player's bus name
/// gaining or losing an owner.
fn rules() -> zbus::Result<(MatchRule<'static>, MatchRule<'static>)> {
    let changed = MatchRule::builder()
        .msg_type(Type::Signal)
        .interface("org.freedesktop.DBus.Properties")?
        .member("PropertiesChanged")?
        .path(OBJECT)?
        .arg(0, INTERFACE)?
        .build();
    let owners = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender("org.freedesktop.DBus")?
        .member("NameOwnerChanged")?
        .arg0ns("org.mpris.MediaPlayer2")?
        .build();
    Ok((changed, owners))
}

fn relay(messages: MessageIterator, events: Sender<Event>) {
    for message in messages {
        if message.is_err() || events.send(Event::Player).is_err() {
            break;
        }
    }
    let _ = events.send(Event::Lost);
}

/// Signal Event::Sound whenever `pactl subscribe` reports a sink or server
/// change. wpctl has no way to watch, but PipeWire's pulse server does.
fn watch_sound(events: Sender<Event>) {
    thread::spawn(move || {
        let child = Command::new("pactl")
            .arg("subscribe")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();

        if let Ok(mut child) = child {
            if let Some(stdout) = child.stdout.take() {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if audible(&line) && events.send(Event::Sound).is_err() {
                        break;
                    }
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = events.send(Event::Lost);
    });
}

/// Whether a `pactl subscribe` line can change the default sink's mute
/// state, e.g. "Event 'change' on sink #679" or "Event 'change' on server #-1".
fn audible(line: &str) -> bool {
    line.contains(" on sink #") || line.contains(" on server")
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
            let _ = Command::new("wpctl")
                .args(["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])
                .status();
            return;
        }
    };

    if let Some(proxy) = connection.and_then(player) {
        let _: zbus::Result<()> = proxy.call(method, &());
    }
}

/// Whether there is a player, and whether it is playing.
fn playback(connection: Option<&Connection>) -> (bool, bool) {
    match connection.and_then(player) {
        Some(proxy) => {
            let playing = proxy.get_property::<String>("PlaybackStatus").is_ok_and(|state| state == "Playing");
            (true, playing)
        }
        None => (false, false),
    }
}

fn muted() -> bool {
    Command::new("wpctl")
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
    fn audible_picks_sink_and_server_changes() {
        assert!(audible("Event 'change' on sink #679"));
        assert!(audible("Event 'change' on server #-1"));
        assert!(!audible("Event 'change' on sink-input #45"));
        assert!(!audible("Event 'new' on client #45496"));
        assert!(!audible("Event 'change' on card #678"));
        assert!(!audible("Event 'change' on source #12"));
    }

    #[test]
    fn pending_gathers_a_burst() {
        let (sender, receiver) = channel();
        for event in [Event::Player, Event::Act(Action::Next), Event::Player, Event::Act(Action::PlayPause)] {
            sender.send(event).unwrap();
        }
        let pending = wait(&receiver, None).unwrap();
        assert_eq!(pending.actions.len(), 2);
        assert!(pending.player);
        assert!(!pending.sound);
        assert!(!pending.lost);
        assert!(receiver.try_recv().is_err(), "everything queued is taken");
    }

    #[test]
    fn pending_rereads_everything_when_a_watcher_is_lost() {
        let mut pending = Pending::default();
        pending.add(Event::Lost);
        assert!(pending.lost && pending.player && pending.sound);
    }

    #[test]
    fn wait_polls_only_when_told_to() {
        let (sender, receiver) = channel::<Event>();
        let pending = wait(&receiver, Some(Duration::from_millis(10))).unwrap();
        assert!(pending.player && pending.sound && pending.actions.is_empty());

        drop(sender);
        assert!(wait(&receiver, None).is_none());
        assert!(wait(&receiver, Some(Duration::from_millis(10))).is_none());
    }

    #[test]
    fn rules_are_valid() {
        // An invalid rule would quietly drop the worker into polling.
        let (changed, owners) = rules().unwrap();
        assert_eq!(
            changed.to_string(),
            "type='signal',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged',path='/org/mpris/MediaPlayer2',arg0='org.mpris.MediaPlayer2.Player'"
        );
        assert!(owners.to_string().contains("arg0namespace='org.mpris.MediaPlayer2'"));
    }

    #[test]
    fn silenced_reads_wpctl_output() {
        assert!(silenced("Volume: 0.40 [MUTED]\n"));
        assert!(!silenced("Volume: 0.40\n"));
        assert!(!silenced(""));
    }
}
