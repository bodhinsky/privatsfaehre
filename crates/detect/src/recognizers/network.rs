use crate::util::{before, lazy_regex};
use crate::{DetectError, Detection, EntityType, Recognizer};
use std::net::{Ipv4Addr, Ipv6Addr};

lazy_regex!(V4, r"\b(?:\d{1,3}\.){3}\d{1,3}\b");
lazy_regex!(V6, r"(?i)(?:[0-9a-f]{1,4}:){1,7}(?:(?::[0-9a-f]{1,4}){1,7}|[0-9a-f]{1,4}|:)");
lazy_regex!(VERSION_BEFORE, r"(?i)(?:\bv|version\s*|ver\.\s*)$");

/// IPv4/IPv6 addresses. Internal ranges (RFC 1918, loopback, link-local,
/// CGNAT, ULA) score higher because they reveal internal infrastructure.
///
/// Uses `std::net` address *types* for parsing only; no sockets are opened.
#[derive(Debug, Default, Clone, Copy)]
pub struct IpRecognizer;

fn v4_internal(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_private() || ip.is_loopback() || ip.is_link_local() || (o[0] == 100 && (64..128).contains(&o[1]))
}

fn v6_internal(ip: Ipv6Addr) -> bool {
    let s = ip.segments()[0];
    ip.is_loopback() || (s & 0xfe00) == 0xfc00 || (s & 0xffc0) == 0xfe80
}

impl Recognizer for IpRecognizer {
    fn id(&self) -> &str {
        "ip"
    }
    fn entities(&self) -> &[EntityType] {
        &[EntityType::IpAddress]
    }
    fn detect(&self, text: &str, out: &mut Vec<Detection>) -> Result<(), DetectError> {
        let bytes = text.as_bytes();
        for m in V4.find_iter(text) {
            let Ok(ip) = m.as_str().parse::<Ipv4Addr>() else { continue };
            let prev = m.start().checked_sub(1).map(|i| bytes[i]);
            let next_is_dot_digit =
                bytes.get(m.end()) == Some(&b'.') && bytes.get(m.end() + 1).is_some_and(u8::is_ascii_digit);
            if prev == Some(b'.') || next_is_dot_digit {
                continue;
            }
            let (score, id) = if VERSION_BEFORE.is_match(before(text, m.start(), 10)) {
                (0.2, "ip.v4")
            } else if ip.is_unspecified() || ip.is_broadcast() || o_is_netmask(ip) {
                (0.3, "ip.v4")
            } else if v4_internal(ip) {
                (0.9, "ip.internal")
            } else {
                (0.75, "ip.public")
            };
            out.push(Detection::new(m.start(), m.end(), EntityType::IpAddress, score, id));
        }
        for m in V6.find_iter(text) {
            let s = m.as_str();
            if s.matches(':').count() < 2 {
                continue;
            }
            // Must not be glued to other hex/word characters.
            let prev_word = text[..m.start()].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == ':');
            let next_word = text[m.end()..].chars().next().is_some_and(|c| c.is_alphanumeric());
            if prev_word || next_word {
                continue;
            }
            let Ok(ip) = s.parse::<Ipv6Addr>() else { continue };
            let (score, id) = if v6_internal(ip) { (0.9, "ip.internal") } else { (0.75, "ip.public") };
            out.push(Detection::new(m.start(), m.end(), EntityType::IpAddress, score, id));
        }
        Ok(())
    }
}

fn o_is_netmask(ip: Ipv4Addr) -> bool {
    let v = u32::from(ip);
    v != 0 && v.leading_ones() + v.trailing_zeros() == 32 && ip.octets()[0] == 255
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recognizers::test_util::run;

    #[test]
    fn v4() {
        let r = IpRecognizer;
        let t = "DB auf 10.0.12.7, Gateway 192.168.1.1, public 8.8.8.8, mask 255.255.255.0, kein 300.1.1.1";
        let d = run(&r, t);
        let got: Vec<_> = d.iter().filter(|d| d.score >= 0.5).map(|d| (d.text(t), d.recognizer.as_ref())).collect();
        assert_eq!(got, [("10.0.12.7", "ip.internal"), ("192.168.1.1", "ip.internal"), ("8.8.8.8", "ip.public")]);
    }

    #[test]
    fn v6_and_versions() {
        let r = IpRecognizer;
        let t = "Host fe80::1ff:fe23:4567:890a und 2001:db8::8a2e:370:7334, Zeit 12:30:45, v1.2.3.4";
        let got: Vec<_> = run(&r, t).into_iter().filter(|d| d.score >= 0.5).map(|d| d.text(t).to_owned()).collect();
        assert_eq!(got, ["fe80::1ff:fe23:4567:890a", "2001:db8::8a2e:370:7334"]);
    }
}
