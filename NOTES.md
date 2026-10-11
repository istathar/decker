# Notes

## Design

Dial detents can arrive much faster than a Key Light can be written to over
the network, so each light gets a worker thread fed by a channel. A burst
collapses to the latest state after a 40 ms debounce, and only that final
state is sent. The worker does all of its light's network traffic, so the
event loop never waits on the network, even when a light is unreachable.

The worker also re-reads its light after 30 seconds without a write, so
changes made elsewhere (by other applications or manual commands) still show
up. very read and write are reported back over a second channel. Changes are
numbered; a report is applied only if nothing newer has been sent since, so a
re-read never undoes a dial turn still on its way to the light. Until a light
has been read once, input for it is ignored rather than sent.

## Discovery

Key Lights advertise themselves over mDNS as `_elg._tcp`, with the MAC address
in the `id` TXT record and the model in `md`. A background browse keeps a
directory of where each MAC was last seen. A light that has just moved can be
advertised at its old and new address at once until the old record expires, so
an address that wasn't in its previous advertisement is taken as the new one.

## Appearance

Keys show the light itself, lit when it is on, drawn to each model's body:
wide with rounded ends for the "Key Light" and "Key Light MK.2", near-square
for the "Key Light Air", as set by `shape` in the configuration. Numbers are
displayed on the LCD only. The accent is a cold blue; the one exception is the
temperature readout, tinted to the light's actual colour, warm through cool.

## Conflicting Applications

Applications like
[StreamController](https://github.com/StreamController/StreamController) claim
the deck through **libusb** and detach the kernel driver, which makes the
`hidraw` node disappear entirely, so that application and **decker** can't run
at once. You may need to disable its autostart entry if present, for example
by renaming to
_~/.config/autostart/com.core447.StreamController.desktop.disabled_.
