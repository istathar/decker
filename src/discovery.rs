use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use mdns_sd::{ResolvedService, ServiceDaemon, ServiceEvent};

use crate::config;

/// What Elgato lights advertise themselves as.
const SERVICE: &str = "_elg._tcp.local.";

/// A light as advertised over mDNS.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// MAC address, from the `id` TXT record, upper case.
    pub id: String,
    /// Instance name, e.g. "Elgato Key Light 5301".
    pub name: String,
    /// From the `md` TXT record, e.g. "Elgato Key Light Air 20LAX9901".
    pub model: String,
    /// IPv4 addresses in ascending order; never empty.
    pub addresses: Vec<SocketAddr>,
}

/// Where each light was last seen, by MAC address. Kept current by a
/// background browse, so a light that moves to a new address is followed.
#[derive(Clone, Default)]
pub struct Directory {
    seen: Arc<Mutex<HashMap<String, Sighting>>>,
}

struct Sighting {
    /// The address requests go to.
    address: SocketAddr,
    /// Every address in the latest advertisement, to tell a new one apart.
    advertised: Vec<SocketAddr>,
}

impl Directory {
    /// Browse for as long as the program runs. If mDNS can't be started the
    /// directory stays empty and lights are reached at their configured host.
    pub fn start() -> Self {
        let directory = Directory::default();

        let daemon = match ServiceDaemon::new() {
            Ok(daemon) => daemon,
            Err(problem) => {
                eprintln!("decker: mDNS unavailable, using configured hosts only: {}", problem);
                return directory;
            }
        };
        let events = match daemon.browse(SERVICE) {
            Ok(events) => events,
            Err(problem) => {
                eprintln!("decker: mDNS browse failed, using configured hosts only: {}", problem);
                return directory;
            }
        };

        let shared = directory.clone();
        thread::spawn(move || {
            // Keep the daemon handle alive for as long as its events are read.
            let _daemon = daemon;
            // Removals are ignored: the last address seen is still the best guess.
            while let Ok(event) = events.recv() {
                if let ServiceEvent::ServiceResolved(service) = event {
                    if let Some(found) = found(&service) {
                        shared.record(&found);
                    }
                }
            }
        });

        directory
    }

    pub fn record(&self, found: &Found) {
        let mut seen = self.seen.lock().unwrap();
        if let Some(address) = prefer(seen.get(&found.id), &found.addresses) {
            let sighting = Sighting { address, advertised: found.addresses.clone() };
            seen.insert(found.id.clone(), sighting);
        }
    }

    pub fn lookup(&self, id: &str) -> Option<SocketAddr> {
        self.seen.lock().unwrap().get(id).map(|sighting| sighting.address)
    }

    /// Block until every id has been seen, or the timeout passes.
    pub fn wait(&self, ids: &[&str], timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while !ids.iter().all(|id| self.lookup(id).is_some()) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

fn found(service: &ResolvedService) -> Option<Found> {
    identify(
        service.get_fullname(),
        service.get_property_val_str("id"),
        service.get_property_val_str("md"),
        service.get_addresses_v4(),
        service.get_port(),
    )
}

/// The parts of an advertisement that matter, apart from mdns-sd's types so
/// they can be tested. None unless it has a valid MAC and an IPv4 address.
fn identify(
    fullname: &str,
    id: Option<&str>,
    model: Option<&str>,
    addresses: impl IntoIterator<Item = Ipv4Addr>,
    port: u16,
) -> Option<Found> {
    let id = config::mac(id?)?;

    let mut addresses: Vec<SocketAddr> = addresses
        .into_iter()
        .map(|ip| SocketAddr::from((ip, port)))
        .collect();
    addresses.sort();
    if addresses.is_empty() {
        return None;
    }

    let name = fullname
        .strip_suffix(SERVICE)
        .and_then(|name| name.strip_suffix('.'))
        .unwrap_or(fullname);

    Some(Found {
        id,
        name: name.to_string(),
        model: model.unwrap_or_default().to_string(),
        addresses,
    })
}

/// Which advertised address to use. A light that has just moved can be
/// advertised at both its old and new address until the old record expires,
/// so an address that wasn't in its last advertisement is taken as the new
/// one. Otherwise keep the address in use, or take the lowest if it's gone or
/// this is the light's first sighting.
fn prefer(previous: Option<&Sighting>, addresses: &[SocketAddr]) -> Option<SocketAddr> {
    let Some(previous) = previous else {
        return addresses.first().copied();
    };

    addresses
        .iter()
        .copied()
        .find(|address| !previous.advertised.contains(address))
        .or_else(|| addresses.contains(&previous.address).then_some(previous.address))
        .or_else(|| addresses.first().copied())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(last: u8) -> SocketAddr {
        SocketAddr::from(([192, 0, 2, last], 9123))
    }

    fn ip(last: u8) -> Ipv4Addr {
        Ipv4Addr::new(192, 0, 2, last)
    }

    fn light(id: &str, addresses: &[u8]) -> Found {
        Found {
            id: id.into(),
            name: "Elgato Key Light 5301".into(),
            model: "Elgato Key Light 20GAK9901".into(),
            addresses: addresses.iter().map(|last| at(*last)).collect(),
        }
    }

    const MAC: &str = "00:00:5E:00:53:01";

    #[test]
    fn identify_reads_an_advertisement() {
        let found = identify(
            "Elgato Key Light 5301._elg._tcp.local.",
            Some("00:00:5e:00:53:01"),
            Some("Elgato Key Light 20GAK9901"),
            [ip(97)],
            9123,
        )
        .unwrap();

        assert_eq!(found, light(MAC, &[97]));
    }

    #[test]
    fn identify_needs_an_id_and_an_address() {
        let name = "Elgato Key Light 5301._elg._tcp.local.";
        assert_eq!(identify(name, None, None, [ip(97)], 9123), None);
        assert_eq!(identify(name, Some("not a mac"), None, [ip(97)], 9123), None);
        assert_eq!(identify(name, Some(MAC), None, [], 9123), None);
    }

    #[test]
    fn identify_sorts_addresses_and_keeps_the_port() {
        let found = identify("odd name", Some(MAC), None, [ip(97), ip(12)], 9999).unwrap();
        assert_eq!(found.addresses, [SocketAddr::from((ip(12), 9999)), SocketAddr::from((ip(97), 9999))]);
        assert_eq!(found.name, "odd name");
        assert_eq!(found.model, "");
    }

    #[test]
    fn directory_records_and_looks_up() {
        let directory = Directory::default();
        assert_eq!(directory.lookup(MAC), None);
        directory.record(&light(MAC, &[97]));
        directory.record(&light("00:00:5E:00:53:02", &[98]));
        assert_eq!(directory.lookup(MAC), Some(at(97)));
        assert_eq!(directory.lookup("00:00:5E:00:53:02"), Some(at(98)));
    }

    #[test]
    fn directory_follows_a_light_that_moves() {
        let directory = Directory::default();
        directory.record(&light(MAC, &[97]));

        // Moved to .200 while .97 is still cached; .200 sorts after it.
        directory.record(&light(MAC, &[97, 200]));
        assert_eq!(directory.lookup(MAC), Some(at(200)));

        // Re-announced unchanged: no flapping back.
        directory.record(&light(MAC, &[97, 200]));
        assert_eq!(directory.lookup(MAC), Some(at(200)));

        // Old record expired.
        directory.record(&light(MAC, &[200]));
        assert_eq!(directory.lookup(MAC), Some(at(200)));

        // Moved again, to a lower address this time.
        directory.record(&light(MAC, &[12, 200]));
        assert_eq!(directory.lookup(MAC), Some(at(12)));
    }

    #[test]
    fn directory_drops_an_address_no_longer_advertised() {
        let directory = Directory::default();
        directory.record(&light(MAC, &[12, 97]));
        assert_eq!(directory.lookup(MAC), Some(at(12)));
        directory.record(&light(MAC, &[97]));
        assert_eq!(directory.lookup(MAC), Some(at(97)));
    }

    #[test]
    fn prefer_without_history_takes_the_lowest() {
        assert_eq!(prefer(None, &[at(97), at(98)]), Some(at(97)));
        assert_eq!(prefer(None, &[]), None);
    }

    #[test]
    fn wait_returns_once_everything_is_seen() {
        let directory = Directory::default();
        directory.record(&light(MAC, &[97]));

        let started = Instant::now();
        directory.wait(&[MAC], Duration::from_secs(10));
        directory.wait(&[], Duration::from_secs(10));
        assert!(started.elapsed() < Duration::from_secs(1));

        let later = directory.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            later.record(&light("AA:BB:CC:DD:EE:FF", &[5]));
        });
        directory.wait(&[MAC, "AA:BB:CC:DD:EE:FF"], Duration::from_secs(10));
        assert!(directory.lookup("AA:BB:CC:DD:EE:FF").is_some());
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn wait_gives_up_at_the_timeout() {
        let directory = Directory::default();
        let started = Instant::now();
        directory.wait(&[MAC], Duration::from_millis(200));
        let waited = started.elapsed();
        assert!(waited >= Duration::from_millis(200) && waited < Duration::from_secs(2));
    }
}
