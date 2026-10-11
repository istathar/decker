# Decker

This is **decker**, a minor daemon program to control Elgato Key Lights and
GNOME media playback from an Elgato [Stream Deck
Plus](https://www.elgato.com/ww/en/p/stream-deck-plus) from Linux.

## Layout

```
 ┌──────┬──────┬──────┬──────┐
 │ prev │ play │ next │ mute │   row 1, MPRIS over D-Bus
 ├──────┼──────┼──────┼──────┤
 │ Air  │ Key  │ MK.2 │ ALL  │   row 2, on/off
 └──────┴──────┴──────┴──────┘
 ╔══════╤══════╤══════╤══════╗
 ║ Air  │ Key  │ MK.2 │ ALL  ║   LCD, one 200 × 100 px zone per dial
 ╚══════╧══════╧══════╧══════╝
    ◉      ◉      ◉      ◉       dials 0-2 one per light
```

Turning a dial changes brightness. Pushing it switches that dial to colour
temperature and back. Pushing dial 3 switches all four together. Clockwise is
more brightness, or more Kelvin.

The deck goes dark once every light is off and nothing has been touched for
10 minutes. While any light is on it stays lit. Any key, dial or touch wakes
it, and that first input only wakes it, so a blind press can't toggle a light.
A light switched on from elsewhere wakes it too.

## Configuration

Lights are listed in _~/.config/decker/lights.conf_, one section per light,
left to right, at most three. You can start from the included
_lights.conf.example_, or seed a configuration from what `--discover` prints:

```
[Key]
id = 00:00:5E:00:53:01

[Air]
host = 192.0.2.81
shape = air
```

`id` is the light's MAC address. A light configured by `id` is found over mDNS
wherever it is on the network, and followed if its address changes. `host` is
a fixed address, used until (or unless) the light is found by `id`. Each light
needs one or the other; giving both covers a light that is slow to answer
mDNS. `shape` is `panel` (Key Light, MK.2) or `air`, and defaults to `panel`.
With fewer than three lights the unused keys and zones are left blank.

`--discover` listens for 10 s and prints a section for each light that answers,
including its `id` and `shape`, named after the end of its advertised name.
Rename the sections to taste and put them in the order the lights stand.

## Build and run

```
cargo build --release
./target/release/decker --list
./target/release/decker
```

`--discover` finds lights (see above), `--brightness PERCENT` sets the panel's
own brightness, `--verbose` prints device events, and `--preview DIR` renders
every key and LCD zone to PNG so the layout can be worked on without the
hardware attached.

## Device access

In order for your user to be able to control the device you need a udev rule  installed to `/etc/udev/rules.d/`. 
_udev/99-elgato-streamdeck.rules_ is provided for this purpose. Without it, the `hidraw` node is root-only. Replug the Stream Deck after installing this file.

Note that only one program may hold a device like this, so make sure you quit
any other Stream Deck controller programs you might have installed first.

## Running at login

If you want the _decker_ daemon to run whenever the Stream Deck is attached, you can install the provided systemd user unit. The udev rule above starts it when the deck is plugged in, or when you log in with it attached, and stops it when the deck is unplugged.

```
cargo build --release
install -D -m 644 lights.conf.example ~/.config/decker/lights.conf   # then edit
install -D target/release/decker ~/.local/bin/decker
install -D -m 644 systemd/decker.service ~/.local/share/systemd/user/decker.service
systemctl --user daemon-reload
systemctl --user start decker.service    # or replug the deck
journalctl --user -u decker -f
```
