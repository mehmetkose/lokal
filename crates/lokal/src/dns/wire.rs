//! Minimal DNS wire format (RFC 1035): parse one question, build an
//! authoritative answer. Also builds queries (doctor's root-zone check).

use std::collections::HashSet;

pub const TYPE_A: u16 = 1;
pub const TYPE_NS: u16 = 2;
pub const TYPE_SOA: u16 = 6;
pub const TYPE_AAAA: u16 = 28;
pub const TYPE_ANY: u16 = 255;
pub const CLASS_IN: u16 = 1;

pub const RCODE_NOERROR: u8 = 0;
pub const RCODE_FORMERR: u8 = 1;
pub const RCODE_NXDOMAIN: u8 = 3;
pub const RCODE_NOTIMP: u8 = 4;
pub const RCODE_REFUSED: u8 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// Lowercase, no trailing dot.
    pub name: String,
    pub qtype: u16,
    pub qclass: u16,
    /// End offset of the question section in the message.
    pub end: usize,
}

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]))
}

/// Parses the single question of a query. `None` if malformed.
pub fn parse_question(msg: &[u8]) -> Option<Question> {
    if msg.len() < 12 || u16_at(msg, 4)? != 1 {
        return None;
    }
    let mut i = 12;
    let mut labels: Vec<String> = vec![];
    let mut total = 0usize;
    loop {
        let len = *msg.get(i)? as usize;
        i += 1;
        if len == 0 {
            break;
        }
        // No compression pointers in a question we accept.
        if len & 0xC0 != 0 || len > 63 {
            return None;
        }
        let label = msg.get(i..i + len)?;
        total += len + 1;
        if total > 255 {
            return None;
        }
        labels.push(String::from_utf8_lossy(label).to_ascii_lowercase());
        i += len;
    }
    let qtype = u16_at(msg, i)?;
    let qclass = u16_at(msg, i + 2)?;
    Some(Question {
        name: labels.join("."),
        qtype,
        qclass,
        end: i + 4,
    })
}

fn push_name(out: &mut Vec<u8>, name: &str) {
    for l in name.split('.').filter(|l| !l.is_empty()) {
        out.push(l.len() as u8);
        out.extend_from_slice(l.as_bytes());
    }
    out.push(0);
}

fn header(id: [u8; 2], rd: bool, rcode: u8, qd: u16, an: u16, ns: u16) -> Vec<u8> {
    // QR=1, opcode 0, AA=1, TC=0, RD copied; RA=0, RCODE.
    let flags1 = 0x84 | u8::from(rd);
    let mut h = vec![id[0], id[1], flags1, rcode & 0x0F];
    for c in [qd, an, ns, 0] {
        h.extend_from_slice(&c.to_be_bytes());
    }
    h
}

fn push_soa(out: &mut Vec<u8>, zone: &str) {
    push_name(out, zone);
    out.extend_from_slice(&TYPE_SOA.to_be_bytes());
    out.extend_from_slice(&CLASS_IN.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // TTL 0
    let mut rdata = vec![];
    push_name(&mut rdata, &format!("ns.{zone}"));
    push_name(&mut rdata, &format!("hostmaster.{zone}"));
    // serial, refresh, retry, expire, minimum (negative-cache TTL) = 0
    for v in [1u32, 3600, 600, 86400, 0] {
        rdata.extend_from_slice(&v.to_be_bytes());
    }
    out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
    out.extend_from_slice(&rdata);
}

/// The authoritative response to `query` for `zone`, given the configured
/// names. `None` for messages that get no reply (responses, garbage).
pub fn answer(query: &[u8], zone: &str, names: &HashSet<String>) -> Option<Vec<u8>> {
    if query.len() < 12 || query[2] & 0x80 != 0 {
        return None; // not a query
    }
    let id = [query[0], query[1]];
    let rd = query[2] & 0x01 != 0;
    let opcode = (query[2] >> 3) & 0x0F;
    let Some(q) = parse_question(query) else {
        return Some(header(id, rd, RCODE_FORMERR, 0, 0, 0));
    };
    let question = &query[12..q.end];
    let reply = |rcode: u8, answers: &[(u16, Vec<u8>)], soa: bool| {
        let mut out = header(id, rd, rcode, 1, answers.len() as u16, u16::from(soa));
        out.extend_from_slice(question);
        for (t, rdata) in answers {
            out.extend_from_slice(&[0xC0, 0x0C]); // pointer to the question name
            out.extend_from_slice(&t.to_be_bytes());
            out.extend_from_slice(&CLASS_IN.to_be_bytes());
            out.extend_from_slice(&0u32.to_be_bytes()); // TTL 0
            out.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
            out.extend_from_slice(rdata);
        }
        if soa {
            push_soa(&mut out, zone);
        }
        out
    };
    if opcode != 0 {
        return Some(header(id, rd, RCODE_NOTIMP, 0, 0, 0));
    }
    let in_zone = q.name == zone || q.name.ends_with(&format!(".{zone}"));
    if !in_zone || q.qclass != CLASS_IN {
        return Some(reply(RCODE_REFUSED, &[], false));
    }
    if q.name == zone {
        if q.qtype == TYPE_SOA {
            let mut rr = vec![];
            push_soa(&mut rr, zone);
            // Re-encode as an answer with the apex owner name (pointer).
            let mut out = header(id, rd, RCODE_NOERROR, 1, 1, 0);
            out.extend_from_slice(question);
            out.extend_from_slice(&[0xC0, 0x0C]);
            let owner_len = zone.split('.').map(|l| l.len() + 1).sum::<usize>() + 1;
            out.extend_from_slice(&rr[owner_len..]);
            return Some(out);
        }
        return Some(reply(RCODE_NOERROR, &[], true));
    }
    // Every name under the zone answers loopback, configured or not.
    // mDNSResponder caches *any* failed lookup for ~75s whatever the SOA
    // says (measured: NXDOMAIN, NODATA, SERVFAIL, REFUSED alike), so a name
    // looked up before `service add` would stay broken. Unconfigured names
    // still don't work: no certificate (TLS refused), HTTP 404.
    let _ = names;
    let mut six = vec![0u8; 16];
    six[15] = 1;
    let answers = match q.qtype {
        TYPE_A => vec![(TYPE_A, vec![127, 0, 0, 1])],
        TYPE_AAAA => vec![(TYPE_AAAA, six)],
        TYPE_ANY => vec![(TYPE_A, vec![127, 0, 0, 1]), (TYPE_AAAA, six)],
        _ => vec![],
    };
    let soa = answers.is_empty();
    Some(reply(RCODE_NOERROR, &answers, soa))
}

/// A query for `name`/`qtype` with recursion desired. (Root servers ignore
/// RD; networks that intercept port 53 answer SERVFAIL without it.)
pub fn query(id: u16, name: &str, qtype: u16) -> Vec<u8> {
    let mut q = id.to_be_bytes().to_vec();
    q.extend_from_slice(&[0x01, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
    push_name(&mut q, name);
    q.extend_from_slice(&qtype.to_be_bytes());
    q.extend_from_slice(&CLASS_IN.to_be_bytes());
    q
}

/// `(rcode, answer count)` of a response.
pub fn response_summary(msg: &[u8]) -> Option<(u8, u16)> {
    if msg.len() < 12 || msg[2] & 0x80 == 0 {
        return None;
    }
    Some((msg[3] & 0x0F, u16_at(msg, 6)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> HashSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// Answer records as (type, ttl, rdata), plus the authority count.
    /// (rcode, answers as (type, ttl, rdata), authority count, AA flag)
    type Decoded = (u8, Vec<(u16, u32, Vec<u8>)>, u16, bool);

    fn decode(resp: &[u8]) -> Decoded {
        let rcode = resp[3] & 0x0F;
        let an = u16_at(resp, 6).unwrap();
        let ns = u16_at(resp, 8).unwrap();
        let aa = resp[2] & 0x04 != 0;
        let q = parse_question(resp).unwrap();
        let mut i = q.end;
        let mut out = vec![];
        for _ in 0..an {
            assert_eq!(&resp[i..i + 2], &[0xC0, 0x0C]);
            let t = u16_at(resp, i + 2).unwrap();
            let ttl = u32::from_be_bytes(resp[i + 6..i + 10].try_into().unwrap());
            let len = u16_at(resp, i + 10).unwrap() as usize;
            out.push((t, ttl, resp[i + 12..i + 12 + len].to_vec()));
            i += 12 + len;
        }
        (rcode, out, ns, aa)
    }

    #[test]
    fn a_and_aaaa_for_configured_names_with_ttl_0() {
        let z = names(&["api.shop.lokal", "shop.lokal"]);
        let (rc, ans, ns, aa) =
            decode(&answer(&query(7, "API.Shop.Lokal", TYPE_A), "lokal", &z).unwrap());
        assert_eq!((rc, ns, aa), (RCODE_NOERROR, 0, true));
        assert_eq!(ans, vec![(TYPE_A, 0, vec![127, 0, 0, 1])]);
        let (_, ans, _, _) =
            decode(&answer(&query(8, "api.shop.lokal", TYPE_AAAA), "lokal", &z).unwrap());
        let mut six = vec![0u8; 16];
        six[15] = 1;
        assert_eq!(ans, vec![(TYPE_AAAA, 0, six)]);
        let (_, ans, _, _) =
            decode(&answer(&query(9, "shop.lokal", TYPE_ANY), "lokal", &z).unwrap());
        assert_eq!(ans.len(), 2);
    }

    #[test]
    fn id_and_question_are_echoed() {
        let z = names(&["a.lokal"]);
        let r = answer(&query(0xBEEF, "A.lokal", TYPE_A), "lokal", &z).unwrap();
        assert_eq!(&r[0..2], &[0xBE, 0xEF]);
        // The question keeps the client's case (0x20 randomisation).
        assert_eq!(&r[12..12 + 9], b"\x01A\x05lokal\x00");
    }

    #[test]
    fn unconfigured_names_also_answer_loopback() {
        // No negative answers: mDNSResponder would cache them for ~75s.
        let z = names(&["api.shop.lokal"]);
        for n in [
            "nope.shop.lokal",
            "shop.lokal",
            "other.lokal",
            "a.b.c.lokal",
        ] {
            let (rc, ans, ns, _) = decode(&answer(&query(1, n, TYPE_A), "lokal", &z).unwrap());
            assert_eq!((rc, ns), (RCODE_NOERROR, 0), "{n}");
            assert_eq!(ans, vec![(TYPE_A, 0, vec![127, 0, 0, 1])], "{n}");
        }
    }

    #[test]
    fn other_types_are_nodata_with_uncacheable_soa() {
        let z = names(&["api.shop.lokal"]);
        let r = answer(&query(1, "api.shop.lokal", 65), "lokal", &z).unwrap();
        let (rc, ans, ns, _) = decode(&r);
        assert_eq!(
            (rc, ans.len(), ns),
            (RCODE_NOERROR, 0, 1),
            "HTTPS record → NODATA"
        );
        // SOA minimum (last 4 bytes) and TTL are 0.
        assert_eq!(&r[r.len() - 4..], &[0, 0, 0, 0]);
    }

    #[test]
    fn apex_soa_and_out_of_zone() {
        let z = names(&[]);
        let r = answer(&query(1, "lokal", TYPE_SOA), "lokal", &z).unwrap();
        let (rc, ans, _, _) = decode(&r);
        assert_eq!((rc, ans.len(), ans[0].0), (RCODE_NOERROR, 1, TYPE_SOA));
        let (rc, _, _, _) = decode(&answer(&query(1, "example.com", TYPE_A), "lokal", &z).unwrap());
        assert_eq!(rc, RCODE_REFUSED);
        let (rc, _, _, _) = decode(&answer(&query(1, "evil-lokal", TYPE_A), "lokal", &z).unwrap());
        assert_eq!(rc, RCODE_REFUSED);
    }

    #[test]
    fn garbage_and_responses() {
        let z = names(&[]);
        assert!(answer(&[1, 2, 3], "lokal", &z).is_none());
        let mut resp = query(1, "a.lokal", TYPE_A);
        resp[2] |= 0x80;
        assert!(
            answer(&resp, "lokal", &z).is_none(),
            "never answer a response"
        );
        let mut bad = query(1, "a.lokal", TYPE_A);
        bad[12] = 0xC0; // compression pointer in the question
        assert_eq!(answer(&bad, "lokal", &z).unwrap()[3] & 0x0F, RCODE_FORMERR);
        let mut notimp = query(1, "a.lokal", TYPE_A);
        notimp[2] |= 0x10; // opcode 2 (status)
        assert_eq!(
            answer(&notimp, "lokal", &z).unwrap()[3] & 0x0F,
            RCODE_NOTIMP
        );
    }
}
