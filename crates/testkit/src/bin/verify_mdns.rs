//! VERIFY V2 (SPEC §12): are `dns_sd` LocalOnly A+AAAA records visible to
//! `getaddrinfo`, and how fast? Throwaway experiment, kept as documentation.
//!
//! Usage: verify_mdns <unique|shared> [hold-secs] [name]
//!   Registers `<name>.local` and `api.<name>.local` (default name v2-<rand>),
//!   reports callback latency and getaddrinfo timings, holds the records for
//!   `hold-secs` so curl/dscacheutil/Chrome can be pointed at them, then exits
//!   (which removes the records).

use std::ffi::{CString, c_char, c_int, c_void};
use std::net::ToSocketAddrs;
use std::time::{Duration, Instant};

type DNSServiceRef = *mut c_void;
type DNSRecordRef = *mut c_void;
type Callback = extern "C" fn(DNSServiceRef, DNSRecordRef, u32, i32, *mut c_void);

unsafe extern "C" {
    fn DNSServiceCreateConnection(sd: *mut DNSServiceRef) -> i32;
    fn DNSServiceRegisterRecord(
        sd: DNSServiceRef,
        rec: *mut DNSRecordRef,
        flags: u32,
        iface: u32,
        fullname: *const c_char,
        rrtype: u16,
        rrclass: u16,
        rdlen: u16,
        rdata: *const c_void,
        ttl: u32,
        cb: Callback,
        ctx: *mut c_void,
    ) -> i32;
    fn DNSServiceRefSockFD(sd: DNSServiceRef) -> c_int;
    fn DNSServiceRemoveRecord(sd: DNSServiceRef, rec: DNSRecordRef, flags: u32) -> i32;
    fn DNSServiceProcessResult(sd: DNSServiceRef) -> i32;
    fn DNSServiceRefDeallocate(sd: DNSServiceRef);
}

struct Pending {
    left: usize,
    errors: Vec<(String, i32)>,
}

struct Ctx {
    name: String,
    pending: *mut Pending,
}

extern "C" fn reply(_: DNSServiceRef, _: DNSRecordRef, _flags: u32, err: i32, ctx: *mut c_void) {
    let ctx = unsafe { &*(ctx as *const Ctx) };
    let p = unsafe { &mut *ctx.pending };
    p.left = p.left.saturating_sub(1);
    if err != 0 {
        p.errors.push((ctx.name.clone(), err));
    }
}

fn lookup(host: &str) -> (Duration, Vec<String>) {
    let t = Instant::now();
    let addrs: Vec<String> = match (host, 0).to_socket_addrs() {
        Ok(a) => a.map(|a| a.ip().to_string()).collect(),
        Err(e) => vec![format!("ERR {e}")],
    };
    (t.elapsed(), addrs)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flags: u32 = match args.get(1).map(String::as_str) {
        Some("shared") => 0x10,
        _ => 0x20,
    };
    let hold: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut rnd = [0u8; 3];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut rnd).unwrap();
    let base = args
        .get(3)
        .cloned()
        .unwrap_or_else(|| format!("v2-{:02x}{:02x}{:02x}", rnd[0], rnd[1], rnd[2]));
    // VERIFY_SUFFIX=lokal tests a non-.local suffix (custom TLD experiment).
    let suffix = std::env::var("VERIFY_SUFFIX").unwrap_or_else(|_| "local".into());
    let names = [format!("{base}.{suffix}"), format!("api.{base}.{suffix}")];

    let mut sd: DNSServiceRef = std::ptr::null_mut();
    let t0 = Instant::now();
    assert_eq!(unsafe { DNSServiceCreateConnection(&mut sd) }, 0);
    let t_conn = t0.elapsed();
    let pending = Box::into_raw(Box::new(Pending {
        left: names.len() * 2,
        errors: vec![],
    }));
    let mut ctxs = vec![];
    let mut recs: Vec<DNSRecordRef> = vec![];
    let t1 = Instant::now();
    for n in &names {
        let c = CString::new(n.as_str()).unwrap();
        for (rrtype, data) in [
            (1u16, vec![127u8, 0, 0, 1]),
            (28u16, {
                let mut v = vec![0u8; 16];
                v[15] = 1;
                v
            }),
        ] {
            let ctx = Box::into_raw(Box::new(Ctx {
                name: format!("{n}/{rrtype}"),
                pending,
            }));
            ctxs.push(ctx);
            let mut rec: DNSRecordRef = std::ptr::null_mut();
            let err = unsafe {
                DNSServiceRegisterRecord(
                    sd,
                    &mut rec,
                    flags,
                    u32::MAX, // kDNSServiceInterfaceIndexLocalOnly
                    c.as_ptr(),
                    rrtype,
                    1,
                    data.len() as u16,
                    data.as_ptr() as *const c_void,
                    120,
                    reply,
                    ctx as *mut c_void,
                )
            };
            assert_eq!(err, 0, "register {n} {rrtype}");
            recs.push(rec);
        }
    }
    let fd = unsafe { DNSServiceRefSockFD(sd) };
    // The callback decrements `left` inside DNSServiceProcessResult.
    loop {
        if unsafe { (*pending).left } == 0 {
            break;
        }
        let mut pfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let n = unsafe { libc::poll(&mut pfd, 1, 5000) };
        if n <= 0 {
            println!("TIMEOUT waiting for callbacks after {:?}", t1.elapsed());
            break;
        }
        let e = unsafe { DNSServiceProcessResult(sd) };
        assert_eq!(e, 0, "process result");
    }
    let t_cb = t1.elapsed();
    let p = unsafe { &*pending };
    println!(
        "flags={} connect={:?} all-callbacks={:?} errors={:?}",
        if flags == 0x10 { "shared" } else { "unique" },
        t_conn,
        t_cb,
        p.errors
    );
    for n in &names {
        let (d1, a1) = lookup(n);
        let (d2, a2) = lookup(n);
        println!("{n}: first {d1:?} {a1:?}; repeat {d2:?} {a2:?}");
    }
    println!("NAMES {} {}", names[0], names[1]);
    if hold > 0 {
        std::thread::sleep(Duration::from_secs(hold));
    }
    if std::env::var("REMOVE").is_ok() {
        let t = Instant::now();
        for r in &recs {
            assert_eq!(unsafe { DNSServiceRemoveRecord(sd, *r, 0) }, 0);
        }
        println!("removed in {:?}", t.elapsed());
        for i in 0..12 {
            let (d, a) = lookup(&names[1]);
            println!("after remove +{:?} #{i}: {d:?} {a:?}", t.elapsed());
            if a.iter().any(|x| x.starts_with("ERR")) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    unsafe { DNSServiceRefDeallocate(sd) };
    let (d, a) = lookup(&names[1]);
    println!("after dealloc: {} {d:?} {a:?}", names[1]);
    let (d, a) = lookup("nope-never-registered-x1.local");
    println!("negative lookup of never-registered name: {d:?} {a:?}");
}
