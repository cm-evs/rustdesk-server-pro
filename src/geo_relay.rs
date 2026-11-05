//! Nearest-relay (geo) selection support — v27.
//!
//! When several relay servers are configured, the stock rendezvous code
//! rotates through them blindly, which for globally distributed relays
//! can send a session to another continent. This module adds a
//! server-side "nearest wins" policy: both peers' public IPs and each
//! relay's IP are located with an optional MaxMind-format city database,
//! and the relay minimizing the summed great-circle distance to both
//! peers is chosen. Geographic distance is the practical server-side
//! proxy for latency — measuring real client<->relay RTT would need
//! client protocol changes, and the stock client takes whatever relay
//! address the rendezvous server assigns.
//!
//! Relays whose summed distance is within [`NEAR_RATIO`]x +
//! [`NEAR_SLACK_KM`] of the best form a near set and are rotated through,
//! so one region's sessions still spread over its equally-near relays
//! (the user's "根据延迟轮询" requirement). If the database is absent, or
//! neither peer can be located, or no relay can be located, selection
//! returns `None` and the caller keeps plain round-robin.
//!
//! The database is opt-in by file presence: `geoip-city.mmdb` (or a
//! GeoLite2-City / DB-IP city file) in the server directory, or a path
//! in the `RELAY_GEOIP_DB` environment variable. Removing the file
//! restores stock round-robin on the next start.

use std::collections::HashMap;
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::{Mutex, OnceLock};

use hbb_common::log;

/// (latitude, longitude) in degrees.
type Coords = (f64, f64);

const NEAR_RATIO: f64 = 1.3;
const NEAR_SLACK_KM: f64 = 250.0;
const DB_CANDIDATES: [&str; 3] = [
    "geoip-city.mmdb",
    "GeoLite2-City.mmdb",
    "dbip-city-lite.mmdb",
];

static DB: OnceLock<Option<maxminddb::Reader<Vec<u8>>>> = OnceLock::new();
static RELAY_COORDS: OnceLock<Mutex<HashMap<String, Option<Coords>>>> = OnceLock::new();

fn db() -> Option<&'static maxminddb::Reader<Vec<u8>>> {
    match DB.get() {
        Some(Some(r)) => Some(r),
        _ => None,
    }
}

/// Load the geo database once (later calls are no-ops). Called at hbbs
/// startup, before the first relay assignment.
pub fn init_geo_db() {
    DB.get_or_init(|| {
        let path = std::env::var_os("RELAY_GEOIP_DB")
            .map(std::path::PathBuf::from)
            .or_else(find_candidate);
        match path {
            Some(p) => match maxminddb::Reader::open_readfile(&p) {
                Ok(r) => {
                    log::info!("relay geo selection: enabled (db: {})", p.display());
                    Some(r)
                }
                Err(e) => {
                    log::warn!(
                        "relay geo selection: failed to load {} ({e}), round-robin",
                        p.display()
                    );
                    None
                }
            },
            None => {
                log::info!("relay geo selection: no city db in server dir, round-robin");
                None
            }
        }
    });
}

fn find_candidate() -> Option<std::path::PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    DB_CANDIDATES.iter().map(|n| dir.join(n)).find(|p| p.is_file())
}

/// True when a database is loaded and nearest selection is active.
pub fn geo_enabled() -> bool {
    db().is_some()
}

fn is_non_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_unspecified() || v4.is_link_local()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || v6.is_unique_local() || v6.is_unicast_link_local()
        }
    }
}

fn coords_in(reader: &maxminddb::Reader<Vec<u8>>, ip: IpAddr) -> Option<Coords> {
    if is_non_public(ip) {
        return None;
    }
    let city = reader
        .lookup(ip)
        .ok()?
        .decode::<maxminddb::geoip2::City>()
        .ok()??;
    match (city.location.latitude, city.location.longitude) {
        (Some(la), Some(lo)) => Some((la, lo)),
        _ => None,
    }
}

/// Coordinates of a public IP, or None when unknown / not applicable.
pub fn coords_of(ip: IpAddr) -> Option<Coords> {
    db().and_then(|r| coords_in(r, ip))
}

/// Great-circle distance in kilometres.
pub fn haversine_km(a: Coords, b: Coords) -> f64 {
    let dlat = (b.0 - a.0).to_radians();
    let dlon = (b.1 - a.1).to_radians();
    let la1 = a.0.to_radians();
    let la2 = b.0.to_radians();
    let h = (dlat / 2.0).sin().powi(2) + la1.cos() * la2.cos() * (dlon / 2.0).sin().powi(2);
    2.0 * 6371.0 * h.sqrt().asin()
}

fn relay_host(relay: &str) -> &str {
    let s = relay.trim();
    if let Some(rest) = s.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            return &rest[..end];
        }
    }
    match s.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() && !h.contains(':') => h,
        _ => s,
    }
}

fn relay_ip(relay: &str) -> Option<IpAddr> {
    let host = relay_host(relay);
    if let Ok(ip) = host.parse::<IpAddr>() {
        return Some(ip);
    }
    (host, 21117u16)
        .to_socket_addrs()
        .ok()?
        .next()
        .map(|sa| sa.ip())
}

/// Coordinates of a configured relay server string ("host" or
/// "host:port"), resolved and cached on first use.
pub fn relay_coords(relay: &str) -> Option<Coords> {
    let cache = RELAY_COORDS.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(g) = cache.lock() {
        if let Some(v) = g.get(relay) {
            return *v;
        }
    }
    let v = relay_ip(relay).and_then(coords_of);
    if let Ok(mut g) = cache.lock() {
        g.insert(relay.to_owned(), v);
    }
    v
}

/// Pick the index of the relay to offer, or None to let the caller fall
/// back to plain round-robin. `relay_xy` must line up with `relays`.
pub fn pick_nearest(
    relays: &[String],
    relay_xy: &[Option<Coords>],
    pa: Option<Coords>,
    pb: Option<Coords>,
    rotation: usize,
) -> Option<usize> {
    let peers: Vec<Coords> = [pa, pb].into_iter().flatten().collect();
    if peers.is_empty() || relays.is_empty() || relay_xy.len() != relays.len() {
        return None;
    }
    let mut scored: Vec<(usize, f64)> = Vec::new();
    for (i, xy) in relay_xy.iter().enumerate() {
        if let Some(xy) = xy {
            scored.push((i, peers.iter().map(|p| haversine_km(*p, *xy)).sum()));
        }
    }
    if scored.is_empty() {
        return None;
    }
    let best = scored.iter().map(|s| s.1).fold(f64::INFINITY, f64::min);
    let near: Vec<usize> = scored
        .iter()
        .filter(|s| s.1 <= best * NEAR_RATIO + NEAR_SLACK_KM)
        .map(|s| s.0)
        .collect();
    near.get(rotation % near.len()).copied()
}

/// Full selection used by the rendezvous server: locate both peers and
/// every configured relay, then pick the nearest.
pub fn pick_for(relays: &[String], pa: IpAddr, pb: IpAddr, rotation: usize) -> Option<usize> {
    if !geo_enabled() {
        return None;
    }
    let xy: Vec<Option<Coords>> = relays.iter().map(|r| relay_coords(r)).collect();
    pick_nearest(relays, &xy, coords_of(pa), coords_of(pb), rotation)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: Coords = (31.23, 121.47);
    const BJS: Coords = (39.90, 116.40);
    const TYO: Coords = (35.68, 139.69);
    const LAX: Coords = (34.05, -118.24);

    fn relays(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("r{i}")).collect()
    }

    #[test]
    fn haversine_known_distance() {
        let d = haversine_km(SHA, BJS);
        assert!(d > 1000.0 && d < 1150.0, "SHA-BJS {d}");
        assert_eq!(haversine_km(SHA, SHA), 0.0);
    }

    #[test]
    fn nearest_wins() {
        let xy = vec![Some(TYO), Some(LAX), Some(SHA)];
        for rot in 0..6 {
            assert_eq!(
                pick_nearest(&relays(3), &xy, Some(SHA), Some(SHA), rot),
                Some(2),
                "rotation {rot}"
            );
        }
    }

    #[test]
    fn near_set_rotates_and_excludes_far() {
        // Two relays in the peers' city, one across the Pacific: the two
        // near ones must alternate, the far one must never be picked.
        let xy = vec![Some(SHA), Some((31.0, 121.2)), Some(LAX)];
        let mut seen = std::collections::HashSet::new();
        for rot in 0..8 {
            let i = pick_nearest(&relays(3), &xy, Some(SHA), Some(SHA), rot);
            assert!(i == Some(0) || i == Some(1), "rotation {rot} -> {i:?}");
            if let Some(i) = i {
                seen.insert(i);
            }
        }
        assert_eq!(seen.len(), 2);
    }

    #[test]
    fn one_sided_peers_still_pick() {
        let xy = vec![Some(TYO), Some(LAX)];
        assert_eq!(
            pick_nearest(&relays(2), &xy, Some(SHA), None, 0),
            Some(0)
        );
    }

    #[test]
    fn fallbacks_return_none() {
        let xy = vec![Some(TYO), Some(LAX)];
        assert_eq!(pick_nearest(&relays(2), &xy, None, None, 0), None);
        let none_xy = vec![None, None];
        assert_eq!(
            pick_nearest(&relays(2), &none_xy, Some(SHA), Some(SHA), 0),
            None
        );
        assert_eq!(pick_nearest(&relays(2), &xy[..1], Some(SHA), None, 0), None);
        assert_eq!(pick_nearest(&[], &[], Some(SHA), None, 0), None);
    }

    #[test]
    fn relay_host_parsing() {
        assert_eq!(relay_host("1.2.3.4:21117"), "1.2.3.4");
        assert_eq!(relay_host("example.com"), "example.com");
        assert_eq!(relay_host("example.com:21117"), "example.com");
        assert_eq!(relay_host("[2001:db8::1]:21117"), "2001:db8::1");
        assert_eq!(relay_host("2001:db8::1"), "2001:db8::1");
    }

    #[test]
    fn real_db_lookup() {
        // Uses the operator-downloaded city db when present in the
        // workspace; skips silently otherwise.
        let p = std::path::Path::new("../geoip/GeoLite2-City.mmdb");
        if !p.is_file() {
            return;
        }
        let r = match maxminddb::Reader::open_readfile(p) {
            Ok(r) => r,
            Err(_) => return,
        };
        // A Guangdong Telecom IP.
        let c = coords_in(&r, "121.8.0.1".parse::<IpAddr>().unwrap_or(IpAddr::from([0, 0, 0, 0])));
        let (la, lo) = c.unwrap_or((0.0, 0.0));
        assert!(c.is_some() && (20.0..27.0).contains(&la) && (110.0..117.0).contains(&lo), "121.8.0.1 -> {c:?}");
        assert!(coords_in(&r, IpAddr::from([192, 168, 1, 1])).is_none());
        assert!(coords_in(&r, "8.8.8.8".parse::<IpAddr>().unwrap_or(IpAddr::from([0, 0, 0, 0]))).is_some());
    }
}
