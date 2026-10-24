//! `mdns-local-only` name backend (SPEC §7, VERIFY V2): A `127.0.0.1` and
//! AAAA `::1` records per FQDN, registered with mDNSResponder on
//! `kDNSServiceInterfaceIndexLocalOnly` through the `dns_sd` C API in
//! libSystem. LocalOnly records never leave this Mac.
//!
//! `dns_sd` refs are not thread-safe, so one dedicated thread (with its own
//! single-threaded tokio runtime) owns the connection and every record.
//! `apply` returns once each new name's register callbacks have reported,
//! never after a sleep.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{CString, c_char, c_int, c_void};
use std::os::fd::{AsRawFd, RawFd};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use tokio::io::unix::AsyncFd;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::platform::{BoxFuture, NamePublisher, NameResult, NameStates};

pub const BACKEND: &str = "mdns-local-only";

#[allow(non_camel_case_types, non_upper_case_globals, dead_code)]
mod ffi {
    use super::*;

    pub type DNSServiceRef = *mut c_void;
    pub type DNSRecordRef = *mut c_void;
    pub type DNSServiceRegisterRecordReply =
        extern "C" fn(DNSServiceRef, DNSRecordRef, u32, i32, *mut c_void);

    pub const kDNSServiceFlagsUnique: u32 = 0x20;
    pub const kDNSServiceInterfaceIndexLocalOnly: u32 = u32::MAX;
    pub const kDNSServiceType_A: u16 = 1;
    pub const kDNSServiceType_AAAA: u16 = 28;
    pub const kDNSServiceClass_IN: u16 = 1;
    pub const kDNSServiceErr_NoError: i32 = 0;
    pub const kDNSServiceErr_NameConflict: i32 = -65548;
    pub const kDNSServiceErr_ServiceNotRunning: i32 = -65563;
    pub const kDNSServiceErr_PolicyDenied: i32 = -65570;

    unsafe extern "C" {
        pub fn DNSServiceCreateConnection(sd: *mut DNSServiceRef) -> i32;
        pub fn DNSServiceRegisterRecord(
            sd: DNSServiceRef,
            rec: *mut DNSRecordRef,
            flags: u32,
            interface_index: u32,
            fullname: *const c_char,
            rrtype: u16,
            rrclass: u16,
            rdlen: u16,
            rdata: *const c_void,
            ttl: u32,
            callback: DNSServiceRegisterRecordReply,
            context: *mut c_void,
        ) -> i32;
        pub fn DNSServiceRemoveRecord(sd: DNSServiceRef, rec: DNSRecordRef, flags: u32) -> i32;
        pub fn DNSServiceRefSockFD(sd: DNSServiceRef) -> c_int;
        pub fn DNSServiceProcessResult(sd: DNSServiceRef) -> i32;
        pub fn DNSServiceRefDeallocate(sd: DNSServiceRef);
    }
}

const TTL: u32 = 120;
/// How long `apply` waits for mDNSResponder before reporting a name failed.
const REGISTER_TIMEOUT: Duration = Duration::from_millis(1500);
const RETRY_FAILED_EVERY: Duration = Duration::from_secs(5);

fn describe(err: i32) -> String {
    match err {
        ffi::kDNSServiceErr_NameConflict => {
            "name conflict: another process registered this name".into()
        }
        ffi::kDNSServiceErr_ServiceNotRunning => "mDNSResponder is not running".into(),
        ffi::kDNSServiceErr_PolicyDenied => {
            "mDNSResponder denied the registration (privacy policy)".into()
        }
        e => format!("dns_sd error {e}"),
    }
}

/// A register callback outcome.
struct Event {
    fqdn: String,
    rrtype: u16,
    err: i32,
}

/// Context passed to the C callback. Freed only after its record has been
/// removed or its connection deallocated (no callback can follow).
struct RecCtx {
    fqdn: String,
    rrtype: u16,
    events: *const RefCell<Vec<Event>>,
}

extern "C" fn register_reply(
    _sd: ffi::DNSServiceRef,
    _rec: ffi::DNSRecordRef,
    _flags: u32,
    err: i32,
    ctx: *mut c_void,
) {
    if ctx.is_null() {
        return;
    }
    // SAFETY: ctx is a live Box<RecCtx> (see RecCtx); callbacks run only on
    // the publisher thread inside DNSServiceProcessResult.
    let ctx = unsafe { &*(ctx as *const RecCtx) };
    // SAFETY: events lives in the Conn that owns this registration.
    let events = unsafe { &*ctx.events };
    events.borrow_mut().push(Event {
        fqdn: ctx.fqdn.clone(),
        rrtype: ctx.rrtype,
        err,
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NameState {
    Pending,
    Registered,
    Failed(String),
}

struct Entry {
    recs: [ffi::DNSRecordRef; 2],
    ctxs: [*mut RecCtx; 2],
    /// Bit 0: A acknowledged, bit 1: AAAA acknowledged.
    acked: u8,
    state: NameState,
}

struct Fd(RawFd);
impl AsRawFd for Fd {
    fn as_raw_fd(&self) -> RawFd {
        self.0
    }
}

struct Conn {
    sd: ffi::DNSServiceRef,
    fd: Option<AsyncFd<Fd>>,
    events: Box<RefCell<Vec<Event>>>,
    entries: BTreeMap<String, Entry>,
}

impl Conn {
    fn open() -> Result<Self> {
        let mut sd: ffi::DNSServiceRef = std::ptr::null_mut();
        // SAFETY: sd is a valid out pointer.
        let err = unsafe { ffi::DNSServiceCreateConnection(&mut sd) };
        if err != ffi::kDNSServiceErr_NoError {
            bail!("cannot connect to mDNSResponder: {}", describe(err));
        }
        // SAFETY: sd is a valid connection.
        let fd = unsafe { ffi::DNSServiceRefSockFD(sd) };
        let afd = match AsyncFd::new(Fd(fd)) {
            Ok(a) => a,
            Err(e) => {
                // SAFETY: sd is valid and not used afterwards.
                unsafe { ffi::DNSServiceRefDeallocate(sd) };
                bail!("cannot watch the mDNSResponder socket: {e}");
            }
        };
        Ok(Self {
            sd,
            fd: Some(afd),
            events: Box::new(RefCell::new(Vec::new())),
            entries: BTreeMap::new(),
        })
    }

    fn register(&mut self, fqdn: &str) -> Result<(), String> {
        let cname = CString::new(fqdn).map_err(|e| e.to_string())?;
        let v4 = [127u8, 0, 0, 1];
        let mut v6 = [0u8; 16];
        v6[15] = 1;
        let mut entry = Entry {
            recs: [std::ptr::null_mut(); 2],
            ctxs: [std::ptr::null_mut(); 2],
            acked: 0,
            state: NameState::Pending,
        };
        for (i, (rrtype, data)) in [
            (ffi::kDNSServiceType_A, &v4[..]),
            (ffi::kDNSServiceType_AAAA, &v6[..]),
        ]
        .into_iter()
        .enumerate()
        {
            let ctx = Box::into_raw(Box::new(RecCtx {
                fqdn: fqdn.to_string(),
                rrtype,
                events: &*self.events,
            }));
            let mut rec: ffi::DNSRecordRef = std::ptr::null_mut();
            // SAFETY: all pointers are valid for the call; ctx outlives the
            // registration (see RecCtx).
            let err = unsafe {
                ffi::DNSServiceRegisterRecord(
                    self.sd,
                    &mut rec,
                    ffi::kDNSServiceFlagsUnique,
                    ffi::kDNSServiceInterfaceIndexLocalOnly,
                    cname.as_ptr(),
                    rrtype,
                    ffi::kDNSServiceClass_IN,
                    data.len() as u16,
                    data.as_ptr() as *const c_void,
                    TTL,
                    register_reply,
                    ctx as *mut c_void,
                )
            };
            if err != ffi::kDNSServiceErr_NoError {
                // SAFETY: the registration failed, so no callback holds ctx.
                drop(unsafe { Box::from_raw(ctx) });
                entry.state = NameState::Failed(describe(err));
                break;
            }
            entry.recs[i] = rec;
            entry.ctxs[i] = ctx;
        }
        let failed = match &entry.state {
            NameState::Failed(r) => Some(r.clone()),
            _ => None,
        };
        if failed.is_some() {
            self.remove_records(&mut entry);
        }
        self.entries.insert(fqdn.to_string(), entry);
        match failed {
            Some(r) => Err(r),
            None => Ok(()),
        }
    }

    fn remove_records(&mut self, e: &mut Entry) {
        for i in 0..2 {
            if !e.recs[i].is_null() {
                // SAFETY: rec belongs to this connection and is removed once.
                let err = unsafe { ffi::DNSServiceRemoveRecord(self.sd, e.recs[i], 0) };
                if err != ffi::kDNSServiceErr_NoError {
                    tracing::warn!("DNSServiceRemoveRecord: {}", describe(err));
                }
                e.recs[i] = std::ptr::null_mut();
            }
            if !e.ctxs[i].is_null() {
                // SAFETY: the record is removed, so no callback can use ctx.
                drop(unsafe { Box::from_raw(e.ctxs[i]) });
                e.ctxs[i] = std::ptr::null_mut();
            }
        }
    }

    fn remove(&mut self, fqdn: &str) {
        if let Some(mut e) = self.entries.remove(fqdn) {
            self.remove_records(&mut e);
        }
    }

    /// Reads every pending reply. Errors mean the connection is broken.
    fn process(&mut self) -> Result<()> {
        let fd = self.fd.as_ref().map(|f| f.get_ref().0).unwrap_or(-1);
        loop {
            // SAFETY: sd is valid; the socket is readable, so this won't block.
            let err = unsafe { ffi::DNSServiceProcessResult(self.sd) };
            if err != ffi::kDNSServiceErr_NoError {
                return Err(anyhow!("{}", describe(err)));
            }
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: pfd is valid; timeout 0 never blocks.
            let n = unsafe { libc::poll(&mut pfd, 1, 0) };
            if n <= 0 || pfd.revents & libc::POLLIN == 0 {
                break;
            }
        }
        let events: Vec<Event> = self.events.borrow_mut().drain(..).collect();
        for ev in events {
            let Some(e) = self.entries.get_mut(&ev.fqdn) else {
                continue;
            };
            if ev.err != ffi::kDNSServiceErr_NoError {
                tracing::warn!(fqdn = %ev.fqdn, "mDNS registration failed: {}", describe(ev.err));
                e.state = NameState::Failed(describe(ev.err));
                continue;
            }
            e.acked |= if ev.rrtype == ffi::kDNSServiceType_A {
                1
            } else {
                2
            };
            if e.acked == 3 && e.state == NameState::Pending {
                e.state = NameState::Registered;
            }
        }
        Ok(())
    }

    /// Waits until the socket is readable (or the deadline passes) and
    /// processes replies. Ok(false) on timeout.
    async fn pump(&mut self, deadline: Option<Instant>) -> Result<bool> {
        let fd = self
            .fd
            .as_ref()
            .ok_or_else(|| anyhow!("connection closed"))?;
        let ready = match deadline {
            Some(d) => match tokio::time::timeout_at(d, fd.readable()).await {
                Ok(r) => r,
                Err(_) => return Ok(false),
            },
            None => fd.readable().await,
        };
        let mut guard = ready?;
        guard.clear_ready();
        drop(guard);
        self.process()?;
        Ok(true)
    }

    fn any_pending(&self, names: &BTreeSet<String>) -> bool {
        names.iter().any(|n| {
            matches!(
                self.entries.get(n).map(|e| &e.state),
                Some(NameState::Pending)
            )
        })
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        // Remove records explicitly: closing the connection alone is
        // asynchronous (VERIFY V2), and names must vanish at once.
        let names: Vec<String> = self.entries.keys().cloned().collect();
        for n in names {
            self.remove(&n);
        }
        // Deregister from the reactor before the socket is closed.
        self.fd = None;
        // SAFETY: sd is valid and never used after this.
        unsafe { ffi::DNSServiceRefDeallocate(self.sd) };
    }
}

enum Cmd {
    Apply(BTreeSet<String>, oneshot::Sender<Vec<NameResult>>),
}

pub struct MdnsPublisher {
    tx: mpsc::UnboundedSender<Cmd>,
    states: NameStates,
}

impl MdnsPublisher {
    /// Connects to mDNSResponder on a dedicated thread.
    pub fn start() -> Result<Self> {
        let states = NameStates::default();
        let (tx, rx) = mpsc::unbounded_channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let st = states.clone();
        std::thread::Builder::new()
            .name("mdns".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = ready_tx.send(Err(anyhow!("runtime: {e}")));
                        return;
                    }
                };
                rt.block_on(actor(rx, st, ready_tx));
            })?;
        ready_rx
            .recv()
            .map_err(|_| anyhow!("mDNS publisher thread died"))??;
        Ok(Self { tx, states })
    }
}

impl NamePublisher for MdnsPublisher {
    fn backend_name(&self) -> &'static str {
        BACKEND
    }

    fn apply<'a>(&'a mut self, fqdns: &'a BTreeSet<String>) -> BoxFuture<'a, Vec<NameResult>> {
        let tx = self.tx.clone();
        let want = fqdns.clone();
        Box::pin(async move {
            let (rtx, rrx) = oneshot::channel();
            if tx.send(Cmd::Apply(want.clone(), rtx)).is_err() {
                return failed_all(&want, "mDNS publisher stopped");
            }
            rrx.await
                .unwrap_or_else(|_| failed_all(&want, "mDNS publisher stopped"))
        })
    }

    fn clear(&mut self) -> BoxFuture<'_, ()> {
        let empty = BTreeSet::new();
        let tx = self.tx.clone();
        Box::pin(async move {
            let (rtx, rrx) = oneshot::channel();
            if tx.send(Cmd::Apply(empty, rtx)).is_ok() {
                let _ = rrx.await;
            }
        })
    }

    fn states(&self) -> NameStates {
        self.states.clone()
    }
}

fn failed_all(names: &BTreeSet<String>, why: &str) -> Vec<NameResult> {
    names
        .iter()
        .map(|n| NameResult {
            fqdn: n.clone(),
            outcome: Err(why.to_string()),
        })
        .collect()
}

/// Brings the connection's records in line with `want` and waits for the
/// new ones to settle.
async fn sync(conn: &mut Conn, want: &BTreeSet<String>, retry_failed: bool) -> Result<()> {
    let stale: Vec<String> = conn
        .entries
        .keys()
        .filter(|k| !want.contains(*k))
        .cloned()
        .collect();
    for n in stale {
        conn.remove(&n);
    }
    let mut fresh = BTreeSet::new();
    for n in want {
        let needs = match conn.entries.get(n).map(|e| &e.state) {
            None => true,
            Some(NameState::Failed(_)) => retry_failed,
            _ => false,
        };
        if needs {
            conn.remove(n);
            let _ = conn.register(n);
            fresh.insert(n.clone());
        }
    }
    let deadline = Instant::now() + REGISTER_TIMEOUT;
    while conn.any_pending(&fresh) {
        if !conn.pump(Some(deadline)).await? {
            break;
        }
    }
    Ok(())
}

fn publish_states(conn: Option<&Conn>, want: &BTreeSet<String>, states: &NameStates) {
    let mut st = states.lock().expect("lock");
    st.clear();
    for n in want {
        let s = match conn.and_then(|c| c.entries.get(n)).map(|e| &e.state) {
            Some(NameState::Registered) => "registered".to_string(),
            Some(NameState::Pending) => "pending".to_string(),
            Some(NameState::Failed(r)) => format!("failed: {r}"),
            None => "failed: not connected to mDNSResponder".to_string(),
        };
        st.insert(n.clone(), s);
    }
}

fn results(conn: Option<&Conn>, want: &BTreeSet<String>) -> Vec<NameResult> {
    want.iter()
        .map(|n| NameResult {
            fqdn: n.clone(),
            outcome: match conn.and_then(|c| c.entries.get(n)).map(|e| &e.state) {
                Some(NameState::Registered) => Ok(()),
                Some(NameState::Pending) => {
                    Err("timed out waiting for mDNSResponder to register the name".into())
                }
                Some(NameState::Failed(r)) => Err(r.clone()),
                None => Err("not connected to mDNSResponder".into()),
            },
        })
        .collect()
}

async fn actor(
    mut rx: mpsc::UnboundedReceiver<Cmd>,
    states: NameStates,
    ready: std::sync::mpsc::Sender<Result<()>>,
) {
    let mut conn = match Conn::open() {
        Ok(c) => {
            let _ = ready.send(Ok(()));
            Some(c)
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let mut want: BTreeSet<String> = BTreeSet::new();
    let mut backoff = Duration::from_millis(100);
    let mut retry = tokio::time::interval(RETRY_FAILED_EVERY);
    retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let mut broken = false;
        tokio::select! {
            cmd = rx.recv() => {
                let Some(Cmd::Apply(set, reply)) = cmd else { break };
                want = set;
                if let Some(c) = conn.as_mut()
                    && let Err(e) = sync(c, &want, true).await
                {
                    tracing::warn!("mDNSResponder connection broke: {e:#}");
                    broken = true;
                }
                if broken {
                    conn = None;
                }
                publish_states(conn.as_ref(), &want, &states);
                let _ = reply.send(results(conn.as_ref(), &want));
            }
            r = async {
                match conn.as_mut() {
                    Some(c) => c.pump(None).await.map(|_| ()),
                    None => std::future::pending().await,
                }
            } => {
                if let Err(e) = r {
                    tracing::warn!("mDNSResponder connection broke: {e:#}");
                    conn = None;
                }
                publish_states(conn.as_ref(), &want, &states);
            }
            _ = retry.tick() => {
                if let Some(c) = conn.as_mut() {
                    let failed = want.iter().any(|n| matches!(c.entries.get(n).map(|e| &e.state), Some(NameState::Failed(_))));
                    if failed {
                        if let Err(e) = sync(c, &want, true).await {
                            tracing::warn!("mDNSResponder connection broke: {e:#}");
                            conn = None;
                        }
                        publish_states(conn.as_ref(), &want, &states);
                    }
                }
            }
        }
        if conn.is_none() {
            // Reconnect with backoff (mDNSResponder restarted) and
            // re-register everything.
            loop {
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(5));
                match Conn::open() {
                    Ok(mut c) => {
                        if sync(&mut c, &want, true).await.is_ok() {
                            tracing::info!(
                                "reconnected to mDNSResponder; {} name(s) re-registered",
                                want.len()
                            );
                            conn = Some(c);
                            backoff = Duration::from_millis(100);
                            break;
                        }
                    }
                    Err(e) => tracing::warn!("{e:#}"),
                }
                // Commands keep flowing while disconnected.
                while let Ok(Cmd::Apply(set, reply)) = rx.try_recv() {
                    want = set;
                    let _ = reply.send(results(None, &want));
                }
            }
            publish_states(conn.as_ref(), &want, &states);
        }
    }
    // Publisher dropped: remove every record (Conn::drop) before exiting.
    drop(conn);
    states.lock().expect("lock").clear();
}
