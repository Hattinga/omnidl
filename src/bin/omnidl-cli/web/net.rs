//! Listen addresses: parsing `--listen`, telling loopback from the network,
//! and the URLs to print at startup.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};

pub const DEFAULT_PORT: u16 = 8080;

/// `127.0.0.1:8080`, `0.0.0.0` (default port), `8081` (localhost), `[::]:8080`, `localhost:8080`.
pub fn parse_listen(s: &str) -> Result<SocketAddr, String> {
    let s = s.trim();
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(addr);
    }
    if let Ok(ip) = s.trim_matches(['[', ']']).parse::<IpAddr>() {
        return Ok(SocketAddr::new(ip, DEFAULT_PORT));
    }
    if let Ok(port) = s.parse::<u16>() {
        return Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port));
    }
    let with_port = if s.contains(':') { s.to_string() } else { format!("{s}:{DEFAULT_PORT}") };
    with_port
        .to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .ok_or_else(|| format!("„{s}“ ist keine gültige Adresse (Beispiel: 0.0.0.0:8080)"))
}

/// Only reachable from this machine; everything else is the network.
pub fn is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Addresses to open in a browser. For `0.0.0.0` / `::` that is every address
/// of this machine that others can reach, plus localhost.
pub fn urls(addr: &SocketAddr) -> Vec<String> {
    let port = addr.port();
    if !addr.ip().is_unspecified() {
        return vec![url(addr.ip(), port)];
    }
    let v6 = addr.is_ipv6();
    let mut ips: Vec<IpAddr> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback() && !i.is_link_local())
        .map(|i| i.ip())
        .filter(|ip| v6 || ip.is_ipv4())
        .collect();
    ips.sort_by_key(|ip| (ip.is_ipv6(), *ip));
    ips.dedup();
    let mut out = vec![format!("http://localhost:{port}")];
    out.extend(ips.into_iter().map(|ip| url(ip, port)));
    out
}

fn url(ip: IpAddr, port: u16) -> String {
    match ip {
        IpAddr::V4(v4) => format!("http://{v4}:{port}"),
        IpAddr::V6(v6) => format!("http://[{v6}]:{port}"),
    }
}

/// Host header of a request that really comes from this machine: `localhost`,
/// `127.x.x.x` or `::1`, with or without port. Guards the password-less mode
/// against DNS rebinding.
pub fn is_local_host(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => host.rsplit_once(':').map_or(host, |(h, p)| if p.parse::<u16>().is_ok() { h } else { host }),
    };
    name.eq_ignore_ascii_case("localhost") || name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_addresses() {
        assert_eq!(parse_listen("127.0.0.1:8081").unwrap(), "127.0.0.1:8081".parse().unwrap());
        assert_eq!(parse_listen("0.0.0.0").unwrap(), "0.0.0.0:8080".parse().unwrap());
        assert_eq!(parse_listen("8085").unwrap(), "127.0.0.1:8085".parse().unwrap());
        assert_eq!(parse_listen("[::]:8082").unwrap(), "[::]:8082".parse().unwrap());
        assert_eq!(parse_listen("::1").unwrap(), "[::1]:8080".parse().unwrap());
        assert!(parse_listen("localhost:8083").unwrap().ip().is_loopback());
        assert!(parse_listen("kein host mit leerzeichen").is_err());
    }

    #[test]
    fn loopback_or_network() {
        for local in ["127.0.0.1:8080", "[::1]:8080", "127.0.0.2:1"] {
            assert!(is_loopback(&local.parse().unwrap()), "{local}");
        }
        for open in ["0.0.0.0:8080", "[::]:8080", "192.168.1.20:8080", "10.0.0.1:80"] {
            assert!(!is_loopback(&open.parse().unwrap()), "{open}");
        }
    }

    #[test]
    fn urls_to_print() {
        assert_eq!(urls(&"127.0.0.1:8081".parse().unwrap()), vec!["http://127.0.0.1:8081"]);
        assert_eq!(urls(&"[::1]:8081".parse().unwrap()), vec!["http://[::1]:8081"]);
        let all = urls(&"0.0.0.0:8082".parse().unwrap());
        assert_eq!(all[0], "http://localhost:8082");
        assert!(all.iter().all(|u| !u.contains("127.0.0.1")), "Loopback nur als localhost: {all:?}");
        assert!(all.iter().skip(1).all(|u| !u.contains('[')), "0.0.0.0 bedient nur IPv4: {all:?}");
    }

    #[test]
    fn local_host_headers() {
        for ok in ["localhost", "localhost:8080", "LOCALHOST:1", "127.0.0.1:8081", "127.0.0.1", "[::1]:8080", "[::1]"] {
            assert!(is_local_host(ok), "{ok}");
        }
        for bad in ["evil.example", "evil.example:8080", "192.168.1.2:8080", "localhost.evil.example", "[::2]:80", ""] {
            assert!(!is_local_host(bad), "{bad}");
        }
    }
}
