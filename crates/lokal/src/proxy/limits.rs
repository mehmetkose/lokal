//! Connection limits (SPEC §9.3). Every local account can reach lokal's
//! loopback listeners, so a connection that isn't doing proxy work must not
//! hold a file descriptor for long:
//! - a client connection with no proxied exchange in flight has a deadline:
//!   `pre_request` until its first request, `idle` after that;
//! - client and upstream sockets are capped separately, below the open-file
//!   limit, with a reserve for the listeners, IPC, logs and name lookups;
//! - when the client cap is full, the connection idle longest is closed to
//!   admit the new one. A new connection is refused only when every admitted
//!   one has a proxied exchange in flight.
//!
//! Only an exchange that reaches an upstream (or an upgraded connection)
//! counts as in flight. Pages lokal builds itself (redirects, 404/421/502
//! pages, the selftest) never do, so a peer that stops reading one can't
//! make its connection exempt.

use std::collections::BTreeMap;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use hyper::body::{Body, Frame, SizeHint};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

/// From accept (port 80) or the end of the TLS handshake (443) to the first request.
pub const PRE_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// No request and no proxied exchange for this long after the first request.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
/// Concurrent upstream name lookups. `getaddrinfo` runs on a blocking
/// thread and outlives a connect that gave up, so it needs its own bound.
pub const MAX_LOOKUPS: usize = 16;
/// The open-file limit the daemon asks for at start (macOS `OPEN_MAX`).
pub const WANT_OPEN_FILES: u64 = 10240;
/// How long `admit` waits for an evicted connection to free its permit
/// before it evicts the next one.
const EVICT_WAIT: Duration = Duration::from_millis(200);
/// At most one "every connection is busy" warning per this many ms.
const SHED_LOG_EVERY_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_clients: usize,
    pub max_upstream: usize,
    pub pre_request: Duration,
    pub idle: Duration,
}

impl Limits {
    /// Splits `open_files` (the soft RLIMIT_NOFILE): a quarter, at least 32
    /// and at most 512, stays free for listeners, IPC, logs, mDNS and name
    /// lookups; the rest is shared equally by client and upstream sockets.
    pub fn for_open_files(open_files: u64) -> Self {
        let n = usize::try_from(open_files)
            .unwrap_or(usize::MAX)
            .min(1 << 20);
        let reserve = (n / 4).clamp(32, 512);
        let each = (n.saturating_sub(reserve) / 2).max(8);
        Self {
            max_clients: each,
            max_upstream: each,
            pre_request: PRE_REQUEST_TIMEOUT,
            idle: IDLE_TIMEOUT,
        }
    }
}

/// The daemon's connection budget, shared by every listener and the
/// upstream connector.
#[derive(Clone)]
pub struct Budget(Arc<Inner>);

struct Inner {
    limits: Limits,
    clients: Arc<Semaphore>,
    upstream: Arc<Semaphore>,
    lookups: Arc<Semaphore>,
    /// Live client connections with no proxied exchange in flight, idle
    /// longest first. A slot is added or removed only under this lock,
    /// together with its `proxied`, `idle_key` and `closed` fields.
    idle: Mutex<BTreeMap<u64, Weak<Slot>>>,
    next_key: AtomicU64,
    epoch: Instant,
    last_shed_log: AtomicU64,
}

impl Budget {
    pub fn new(limits: Limits) -> Self {
        Self(Arc::new(Inner {
            limits,
            clients: Arc::new(Semaphore::new(limits.max_clients)),
            upstream: Arc::new(Semaphore::new(limits.max_upstream)),
            lookups: Arc::new(Semaphore::new(MAX_LOOKUPS)),
            idle: Mutex::new(BTreeMap::new()),
            next_key: AtomicU64::new(1),
            epoch: Instant::now(),
            last_shed_log: AtomicU64::new(0),
        }))
    }

    pub fn limits(&self) -> Limits {
        self.0.limits
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.0.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn idle_map(&self) -> MutexGuard<'_, BTreeMap<u64, Weak<Slot>>> {
        self.0.idle.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A client permit: a free one, or the one freed by closing the
    /// connection idle longest. `None` only when every admitted connection
    /// has a proxied exchange in flight: the new connection is refused.
    pub async fn admit(&self) -> Option<OwnedSemaphorePermit> {
        loop {
            if let Ok(p) = self.0.clients.clone().try_acquire_owned() {
                return Some(p);
            }
            let Some(victim) = self.pop_idle() else {
                self.note_shed();
                return None;
            };
            victim.close.notify_one();
            drop(victim);
            // An idle victim drops its connection at once. One that turned
            // busy after it was picked takes longer: evict the next one.
            if let Ok(Ok(p)) =
                tokio::time::timeout(EVICT_WAIT, self.0.clients.clone().acquire_owned()).await
            {
                return Some(p);
            }
        }
    }

    fn pop_idle(&self) -> Option<Arc<Slot>> {
        let mut idle = self.idle_map();
        while let Some((_, w)) = idle.pop_first() {
            if let Some(s) = w.upgrade() {
                s.idle_key.store(0, Ordering::Release);
                return Some(s);
            }
        }
        None
    }

    fn note_shed(&self) {
        let now = self.now_ms().max(1);
        let last = self.0.last_shed_log.load(Ordering::Relaxed);
        if (last == 0 || now.saturating_sub(last) >= SHED_LOG_EVERY_MS)
            && self
                .0
                .last_shed_log
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            tracing::warn!(
                "all {} client connections have a proxied request in flight; refusing new connections",
                self.0.limits.max_clients
            );
        }
    }

    /// Starts tracking an admitted connection. It can be evicted, and its
    /// `pre_request` deadline runs, until a proxied exchange starts.
    pub fn open(&self, permit: OwnedSemaphorePermit) -> Arc<Slot> {
        let slot = Arc::new(Slot {
            budget: self.clone(),
            close: Notify::new(),
            idle_key: AtomicU64::new(0),
            proxied: AtomicUsize::new(0),
            closed: AtomicBool::new(false),
            last_active: AtomicU64::new(self.now_ms()),
            seen_request: AtomicBool::new(false),
            permit: Mutex::new(Some(permit)),
        });
        let mut idle = self.idle_map();
        slot.make_idle(&mut idle);
        drop(idle);
        slot
    }

    /// Permits for `n` upstream sockets, or `None` when the cap is reached
    /// (the request gets a 503 instead of the daemon running out of fds).
    pub fn upstream(&self, n: u32) -> Option<OwnedSemaphorePermit> {
        self.0.upstream.clone().try_acquire_many_owned(n).ok()
    }

    /// A permit for one upstream name lookup, or `None` when too many run.
    pub fn lookup(&self) -> Option<OwnedSemaphorePermit> {
        self.0.lookups.clone().try_acquire_owned().ok()
    }
}

/// One client connection's place in the budget.
pub struct Slot {
    budget: Budget,
    /// Notified when the connection is evicted.
    close: Notify,
    /// Its key in the idle map while it is there, else 0.
    idle_key: AtomicU64,
    /// Proxied exchanges in flight.
    proxied: AtomicUsize,
    /// The connection task has ended; a late `Busy` drop must not put the
    /// slot back in the idle map.
    closed: AtomicBool,
    /// When a request last started or a proxied exchange last ended, in ms
    /// since the budget's epoch.
    last_active: AtomicU64,
    seen_request: AtomicBool,
    /// The client permit. Released when the connection task ends, unless an
    /// upgrade task took it along with the socket.
    permit: Mutex<Option<OwnedSemaphorePermit>>,
}

impl std::fmt::Debug for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Slot")
            .field("proxied", &self.proxied.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Slot {
    /// Needs the idle map's lock (passed in to prove it).
    fn make_idle(self: &Arc<Self>, idle: &mut BTreeMap<u64, Weak<Slot>>) {
        let key = self.budget.0.next_key.fetch_add(1, Ordering::Relaxed);
        self.idle_key.store(key, Ordering::Release);
        idle.insert(key, Arc::downgrade(self));
    }

    /// Restarts the deadline (the TLS handshake just ended, say).
    pub fn touch(&self) {
        self.last_active
            .store(self.budget.now_ms(), Ordering::Release);
    }

    /// Called for every request the connection makes.
    pub fn request_started(&self) {
        self.seen_request.store(true, Ordering::Release);
        self.touch();
    }

    pub fn seen_request(&self) -> bool {
        self.seen_request.load(Ordering::Acquire)
    }

    /// A proxied exchange starts: until the guard drops, the connection is
    /// busy (no deadline, not evicted).
    pub fn proxied(self: &Arc<Self>) -> Busy {
        let mut idle = self.budget.idle_map();
        if self.proxied.fetch_add(1, Ordering::AcqRel) == 0 {
            let key = self.idle_key.swap(0, Ordering::AcqRel);
            if key != 0 {
                idle.remove(&key);
            }
        }
        drop(idle);
        Busy(self.clone())
    }

    pub fn busy(&self) -> bool {
        self.proxied.load(Ordering::Acquire) > 0
    }

    fn limit(&self) -> Duration {
        let l = self.budget.0.limits;
        if self.seen_request() {
            l.idle
        } else {
            l.pre_request
        }
    }

    /// When the deadline passes if nothing happens before then. While busy,
    /// one period from now (checked again then).
    pub fn deadline(&self) -> Instant {
        if self.busy() {
            return Instant::now() + self.limit();
        }
        let last = Duration::from_millis(self.last_active.load(Ordering::Acquire));
        self.budget.0.epoch + last + self.limit()
    }

    /// Past its deadline: nothing proxied in flight and no activity for
    /// `pre_request` (before the first request) or `idle`.
    pub fn expired(&self) -> bool {
        self.expired_at(self.budget.now_ms())
    }

    fn expired_at(&self, now_ms: u64) -> bool {
        let limit = u64::try_from(self.limit().as_millis()).unwrap_or(u64::MAX);
        !self.busy() && now_ms.saturating_sub(self.last_active.load(Ordering::Acquire)) >= limit
    }

    /// Resolves once the connection has been chosen for eviction.
    pub async fn evicted(&self) {
        self.close.notified().await;
    }

    /// For an upgrade: the socket moves to another task, and the client
    /// permit goes with it.
    pub fn take_permit(&self) -> Option<OwnedSemaphorePermit> {
        self.permit.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// The connection task ended: leave the idle map for good, and release
    /// the client permit unless an upgrade task took it.
    pub fn finish(&self) {
        let mut idle = self.budget.idle_map();
        self.closed.store(true, Ordering::Release);
        let key = self.idle_key.swap(0, Ordering::AcqRel);
        if key != 0 {
            idle.remove(&key);
        }
        drop(idle);
        drop(self.take_permit());
    }
}

/// Calls [`Slot::finish`] when the connection task ends, on every path.
pub struct Finish(pub Arc<Slot>);

impl Drop for Finish {
    fn drop(&mut self) {
        self.0.finish();
    }
}

/// A proxied exchange in flight. Lives as long as the upstream response
/// body, or the upgraded connection.
pub struct Busy(Arc<Slot>);

impl Drop for Busy {
    fn drop(&mut self) {
        let s = &self.0;
        s.touch();
        let mut idle = s.budget.idle_map();
        if s.proxied.fetch_sub(1, Ordering::AcqRel) == 1 && !s.closed.load(Ordering::Acquire) {
            s.make_idle(&mut idle);
        }
    }
}

/// A response body that keeps its exchange busy until hyper drops it.
pub struct BusyBody<B> {
    body: B,
    _busy: Option<Busy>,
}

impl<B> BusyBody<B> {
    pub fn new(body: B, busy: Option<Busy>) -> Self {
        Self { body, _busy: busy }
    }
}

impl<B: Body + Unpin> Body for BusyBody<B> {
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Pin::new(&mut self.body).poll_frame(cx)
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(clients: usize) -> Budget {
        Budget::new(Limits {
            max_clients: clients,
            max_upstream: 4,
            pre_request: Duration::from_millis(100),
            idle: Duration::from_millis(1000),
        })
    }

    #[test]
    fn limits_leave_a_reserve() {
        let l = Limits::for_open_files(10240);
        assert_eq!((l.max_clients, l.max_upstream), (4864, 4864));
        let l = Limits::for_open_files(256);
        assert_eq!((l.max_clients, l.max_upstream), (96, 96));
        let l = Limits::for_open_files(48);
        assert_eq!((l.max_clients, l.max_upstream), (8, 8));
        for n in [48u64, 64, 256, 1024, 10240, 1 << 40] {
            let l = Limits::for_open_files(n);
            let n = usize::try_from(n).unwrap().min(1 << 20);
            assert!(l.max_clients + l.max_upstream + 32 <= n, "{n}");
        }
    }

    /// The longest-idle connection is evicted; a busy one never is; with
    /// every connection busy the new one is refused.
    #[tokio::test]
    async fn admit_evicts_the_longest_idle_and_never_a_busy_one() {
        let b = budget(2);
        let first = b.open(b.admit().await.unwrap());
        let second = b.open(b.admit().await.unwrap());
        let _f1 = Finish(first.clone());
        let _f2 = Finish(second.clone());
        let busy = second.proxied();

        // Full: admitting a third evicts `first`. Its task would drop the
        // connection on `evicted()`; here we finish it by hand.
        let b2 = b.clone();
        let third = tokio::spawn(async move { b2.admit().await });
        tokio::time::timeout(Duration::from_secs(1), first.evicted())
            .await
            .expect("first is evicted");
        first.finish();
        let third = b.open(third.await.unwrap().expect("admitted"));
        let _f3 = Finish(third.clone());

        // `second` is busy and `third` idle: `third` goes next.
        let b2 = b.clone();
        let fourth = tokio::spawn(async move { b2.admit().await });
        tokio::time::timeout(Duration::from_secs(1), third.evicted())
            .await
            .expect("third is evicted, not the busy second");
        third.finish();
        let fourth = b.open(fourth.await.unwrap().expect("admitted"));
        let _f4 = Finish(fourth.clone());

        // Both busy: refused at once.
        let _busy4 = fourth.proxied();
        assert!(b.admit().await.is_none());
        drop(busy);
        assert!(!second.busy());
    }

    /// A guard dropped after the connection ended (an upgrade task, say)
    /// must not put it back in the idle map.
    #[tokio::test]
    async fn a_late_busy_drop_does_not_revive_a_finished_connection() {
        let b = budget(1);
        let s = b.open(b.admit().await.unwrap());
        let busy = s.proxied();
        s.finish();
        drop(busy);
        assert!(b.pop_idle().is_none());
        // Its permit is back.
        assert!(b.admit().await.is_some());
    }

    /// An upgrade task that takes the permit keeps the slot's place.
    #[tokio::test]
    async fn an_upgrade_keeps_the_client_permit() {
        let b = budget(1);
        let s = b.open(b.admit().await.unwrap());
        let permit = s.take_permit();
        s.finish();
        assert!(b.0.clients.available_permits() == 0);
        drop(permit);
        assert!(b.0.clients.available_permits() == 1);
    }

    #[tokio::test]
    async fn deadlines() {
        let b = budget(4);
        let s = b.open(b.admit().await.unwrap());
        let t0 = s.last_active.load(Ordering::Acquire);
        assert!(!s.expired_at(t0 + 99));
        assert!(s.expired_at(t0 + 100), "pre_request");
        s.request_started();
        let t1 = s.last_active.load(Ordering::Acquire);
        assert!(!s.expired_at(t1 + 999));
        assert!(s.expired_at(t1 + 1000), "idle");
        let busy = s.proxied();
        assert!(!s.expired_at(t1 + 1_000_000), "never while busy");
        drop(busy);
        assert!(!s.expired_at(s.last_active.load(Ordering::Acquire) + 999));
        s.finish();
    }
}
