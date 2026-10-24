//! The daemon (`lokal daemon run`, SPEC §8.2): startup, reload
//! orchestration, IPC, selftest, graceful shutdown.

pub mod logging;
pub mod server;

use std::collections::{BTreeSet, HashSet};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use arc_swap::ArcSwap;
use tokio::sync::watch;

use crate::ca::CaCert;
use crate::ca::resolver::CertStore;
use crate::dns::{self, DnsPublisher};
use crate::ipc::{self, Request, SelftestAction};
use crate::paths::Paths;
use crate::platform::{self, NamePublisher, NameResult, NameStates};
use crate::proxy::{self, Budget, ConnTracker, Limits, Proxy, routes::RouteTable};
use crate::state::State;

/// Flags of `lokal daemon run` (tests use them; launchd passes none).
#[derive(Debug, Clone, Default)]
pub struct Options {
    pub base_dir: Option<PathBuf>,
    pub https_port: Option<u16>,
    /// Bind the `.lokal` DNS server directly on 127.0.0.1:<port> (tests).
    pub dns_port: Option<u16>,
    pub http_port: Option<u16>,
    pub name_backend: String,
    pub foreground: bool,
    /// Test overrides for the connection limits (SPEC §9.3).
    pub max_client_connections: Option<usize>,
    pub max_upstream_connections: Option<usize>,
    pub pre_request_timeout: Option<Duration>,
    pub idle_timeout: Option<Duration>,
}

const SELFTEST_TTL: Duration = Duration::from_secs(60);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(1);

pub struct Core {
    pub paths: Paths,
    started: Instant,
    pub routes: Arc<ArcSwap<RouteTable>>,
    pub certs: Arc<CertStore>,
    names: tokio::sync::Mutex<Names>,
    name_states: Vec<NameStates>,
    name_backend: String,
    reload_lock: tokio::sync::Mutex<()>,
    /// The active selftest names (one per TLD).
    selftest: std::sync::Mutex<Vec<String>>,
    listeners: Vec<String>,
    listener_source: &'static str,
}

/// `.lokal` names go to the built-in DNS server; everything else (`.local`)
/// to the platform publisher (mDNS LocalOnly on macOS).
pub struct Names {
    local: Box<dyn NamePublisher>,
    lokal: DnsPublisher,
}

impl Names {
    async fn apply(&mut self, desired: &BTreeSet<String>) -> Vec<NameResult> {
        let (zone, other): (BTreeSet<String>, BTreeSet<String>) =
            desired.iter().cloned().partition(|f| dns::in_zone(f));
        let mut r = self.lokal.apply(&zone).await;
        r.extend(self.local.apply(&other).await);
        r
    }

    async fn clear(&mut self) {
        self.lokal.clear().await;
        self.local.clear().await;
    }
}

impl Core {
    pub async fn handle(self: &Arc<Self>, req: Request) -> serde_json::Value {
        let v = match req {
            Request::Ping => serde_json::to_value(self.ping()),
            Request::Status => serde_json::to_value(self.status()),
            Request::Reload => serde_json::to_value(self.reload().await),
            Request::Selftest { action } => serde_json::to_value(self.selftest(action).await),
        };
        v.unwrap_or_else(|e| serde_json::json!({ "ok": false, "error": e.to_string() }))
    }

    fn ping(&self) -> ipc::Ping {
        ipc::Ping {
            ok: true,
            version: crate::version::VERSION.into(),
            build_id: crate::version::build_id().into(),
            pid: std::process::id(),
        }
    }

    pub fn status(&self) -> ipc::Status {
        let mut merged = std::collections::BTreeMap::new();
        for src in &self.name_states {
            if let Ok(m) = src.lock() {
                merged.extend(m.iter().map(|(k, v)| (k.clone(), v.clone())));
            }
        }
        let names = merged
            .into_iter()
            .map(|(fqdn, state)| ipc::NameStatus { fqdn, state })
            .collect();
        ipc::Status {
            schema: 1,
            setup_done: self.paths.install_record.is_file(),
            running: true,
            pid: Some(std::process::id()),
            version: Some(crate::version::VERSION.into()),
            build_id: Some(crate::version::build_id().into()),
            uptime_secs: Some(self.started.elapsed().as_secs()),
            listeners: self.listeners.clone(),
            listener_source: Some(self.listener_source.into()),
            name_backend: Some(self.name_backend.clone()),
            names,
            certs_cached: self.certs.cached_count(),
        }
    }

    /// Re-reads the state and applies the difference. Returns only when
    /// every configured FQDN is ready (route live, name registered, leaf
    /// issued) or has failed.
    pub async fn reload(&self) -> ipc::Reload {
        let _g = self.reload_lock.lock().await;
        let t = Instant::now();
        let state = match State::load(&self.paths) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("reload: {e:#}");
                return ipc::Reload {
                    ok: false,
                    ms: t.elapsed().as_millis() as u64,
                    error: Some(format!("{e:#}")),
                    ..Default::default()
                };
            }
        };
        let selftest = self.selftest.lock().expect("lock").clone();
        let table = RouteTable::build(&state, &selftest);
        let desired: BTreeSet<String> = table.fqdns();
        let old = self.routes.load_full();
        let removed: Vec<String> = old.fqdns().difference(&desired).cloned().collect();
        let changed = table.changed_since(&old);

        // 1. certificates (pre-warm, so no handshake waits on signing)
        let wanted: Vec<String> = desired.iter().cloned().collect();
        let cert_failures = tokio::task::block_in_place(|| self.certs.prewarm(&wanted));
        let mut allowed: HashSet<String> = desired.iter().cloned().collect();
        allowed.extend(old.fqdns());
        self.certs.set_allowed(allowed);
        // 2. routes
        self.routes.store(Arc::new(table));
        // 3. names: returns once each new name's registration has settled
        let results = self.names.lock().await.apply(&desired).await;
        // 4. drop what is gone
        self.certs.set_allowed(desired.iter().cloned().collect());
        self.certs.evict_unallowed();

        let mut failed: Vec<ipc::Failed> = vec![];
        for (fqdn, err) in cert_failures {
            failed.push(ipc::Failed { fqdn, error: err });
        }
        for r in results {
            if let Err(e) = r.outcome
                && !failed.iter().any(|f| f.fqdn == r.fqdn)
            {
                failed.push(ipc::Failed {
                    fqdn: r.fqdn,
                    error: e,
                });
            }
        }
        failed.sort_by(|a, b| a.fqdn.cmp(&b.fqdn));
        let ready: Vec<String> = desired
            .iter()
            .filter(|f| !failed.iter().any(|x| &x.fqdn == *f))
            .cloned()
            .collect();
        let ms = t.elapsed().as_millis() as u64;
        if !changed.is_empty() || !removed.is_empty() || !failed.is_empty() {
            tracing::info!(
                "reload: {} ready, {} changed, {} removed, {} failed in {ms}ms",
                ready.len(),
                changed.len(),
                removed.len(),
                failed.len()
            );
            for f in &failed {
                tracing::warn!(fqdn = %f.fqdn, "not ready: {}", f.error);
            }
        }
        ipc::Reload {
            ok: true,
            ms,
            ready,
            removed,
            failed,
            error: None,
        }
    }

    async fn selftest(self: &Arc<Self>, action: SelftestAction) -> ipc::Selftest {
        let t = Instant::now();
        match action {
            SelftestAction::Begin => {
                // One throwaway name per TLD, so both resolution paths are tested.
                let base = format!("lokal-selftest-{}", crate::util::random_hex(6));
                let names = vec![format!("{base}.{}", dns::ZONE), format!("{base}.local")];
                *self.selftest.lock().expect("lock") = names.clone();
                let r = self.reload().await;
                // Expire them in case the CLI never says `end`.
                let core = self.clone();
                let n = names.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(SELFTEST_TTL).await;
                    let expired = {
                        let mut s = core.selftest.lock().expect("lock");
                        if *s == n {
                            s.clear();
                            true
                        } else {
                            false
                        }
                    };
                    if expired {
                        core.reload().await;
                    }
                });
                let err = r
                    .failed
                    .iter()
                    .find(|f| names.contains(&f.fqdn))
                    .map(|f| format!("{}: {}", f.fqdn, f.error))
                    .or(r.error);
                ipc::Selftest {
                    ok: err.is_none(),
                    fqdn: names.first().cloned(),
                    fqdns: names,
                    ms: t.elapsed().as_millis() as u64,
                    error: err,
                }
            }
            SelftestAction::End => {
                let had = std::mem::take(&mut *self.selftest.lock().expect("lock"));
                if !had.is_empty() {
                    self.reload().await;
                }
                ipc::Selftest {
                    ok: true,
                    fqdn: had.first().cloned(),
                    fqdns: had,
                    ms: t.elapsed().as_millis() as u64,
                    error: None,
                }
            }
        }
    }
}

fn fmt_addr(a: SocketAddr) -> String {
    match a {
        SocketAddr::V4(v) => format!("{}:{}", v.ip(), v.port()),
        SocketAddr::V6(v) => format!("[{}]:{}", v.ip(), v.port()),
    }
}

fn bind_direct(port: u16) -> Result<Vec<std::net::TcpListener>> {
    let mut v = vec![];
    for ip in [
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
    ] {
        let addr = SocketAddr::new(ip, port);
        match std::net::TcpListener::bind(addr) {
            Ok(l) => v.push(l),
            Err(e) if ip.is_ipv6() && !v.is_empty() => {
                eprintln!("warning: cannot bind {addr}: {e}");
            }
            Err(e) => return Err(e).with_context(|| format!("cannot bind {addr}")),
        }
    }
    Ok(v)
}

/// Entry point of `lokal daemon run`. Returns the exit code.
pub fn run(opts: Options) -> Result<i32> {
    let paths = match &opts.base_dir {
        Some(d) => Paths::sandbox(d.clone()),
        None => Paths::resolve()?,
    };
    paths.ensure_base()?;
    let _log = logging::init(&paths, opts.foreground)?;
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&paths.daemon_lock)
        .with_context(|| format!("cannot open {}", paths.daemon_lock.display()))?;
    if lock.try_lock().is_err() {
        tracing::error!("another lokal daemon is already running (daemon.lock is held)");
        bail!("another lokal daemon is already running");
    }
    // launchd starts agents with a soft limit of 256 open files; the
    // connection caps are sized from what this leaves (SPEC §9.3).
    let open_files = platform::raise_open_file_limit(proxy::limits::WANT_OPEN_FILES);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let res = rt.block_on(main(paths, opts, open_files));
    if let Err(e) = &res {
        tracing::error!("daemon failed: {e:#}");
    }
    drop(lock);
    res
}

fn limits(opts: &Options, open_files: u64) -> Limits {
    let mut l = Limits::for_open_files(open_files);
    if let Some(n) = opts.max_client_connections {
        l.max_clients = n.max(1);
    }
    if let Some(n) = opts.max_upstream_connections {
        l.max_upstream = n.max(1);
    }
    if let Some(d) = opts.pre_request_timeout {
        l.pre_request = d;
    }
    if let Some(d) = opts.idle_timeout {
        l.idle = d;
    }
    l
}

async fn main(paths: Paths, opts: Options, open_files: u64) -> Result<i32> {
    let t0 = Instant::now();
    let platform = platform::current();
    let ca = match CaCert::load(&paths) {
        Ok(c) => c,
        Err(e) => {
            // Under launchd a missing CA would respawn us every second; slow down.
            tracing::error!("no certificate authority ({e:#}); run 'lokal setup'");
            if opts.https_port.is_none() {
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
            return Err(e);
        }
    };
    let certs = Arc::new(CertStore::new(ca.issuer()?));

    // PORTING: on macOS production listeners come from launchd (socket
    // activation); tests bind high ports directly.
    let (https, http, dns_socks, source) = match opts.https_port {
        Some(p) => {
            let http = match opts.http_port {
                Some(hp) => bind_direct(hp)?,
                None => vec![],
            };
            let dns_socks = match opts.dns_port {
                Some(dp) => vec![
                    std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, dp))
                        .with_context(|| format!("cannot bind 127.0.0.1:{dp}/udp"))?,
                ],
                None => vec![],
            };
            (bind_direct(p)?, http, dns_socks, "direct")
        }
        None => match platform.service.inherited_listeners()? {
            Some(l) => (l.https, l.http, l.dns, "launchd"),
            None => bail!(
                "not started by the service manager; pass --https-port (and --http-port) to bind directly"
            ),
        },
    };
    if https.is_empty() {
        bail!("no HTTPS listener");
    }
    let mut listener_desc = vec![];
    for l in https.iter().chain(http.iter()) {
        if let Ok(a) = l.local_addr() {
            listener_desc.push(fmt_addr(a));
        }
    }
    for u in &dns_socks {
        if let Ok(a) = u.local_addr() {
            listener_desc.push(format!("{}/udp", fmt_addr(a)));
        }
    }
    if dns_socks.is_empty() && source == "launchd" {
        tracing::warn!(
            "no DNS socket from launchd: .lokal names won't resolve; run 'lokal upgrade'"
        );
    }

    let backend = match opts.name_backend.as_str() {
        "" | "auto" => platform.default_name_backend,
        "none" => "none",
        other => other,
    };
    let local = platform::name_publisher(backend)?;
    let zone: dns::ZoneNames = Default::default();
    let lokal = DnsPublisher::new(zone.clone());
    let name_states = vec![local.states(), lokal.states()];
    let name_backend = format!("{}+{}", local.backend_name(), lokal.backend_name());
    let names = Names { local, lokal };
    let routes = Arc::new(ArcSwap::from_pointee(RouteTable::default()));
    let core = Arc::new(Core {
        paths: paths.clone(),
        started: Instant::now(),
        routes: routes.clone(),
        certs: certs.clone(),
        names: tokio::sync::Mutex::new(names),
        name_states,
        name_backend: name_backend.clone(),
        reload_lock: tokio::sync::Mutex::new(()),
        selftest: std::sync::Mutex::new(vec![]),
        listeners: listener_desc.clone(),
        listener_source: source,
    });

    // Register every name and pre-warm every cert before serving.
    let r = core.reload().await;
    let (stx, srx) = watch::channel(false);
    let ipc = server::bind(&core)?;
    tokio::spawn(server::serve(ipc, core.clone(), srx.clone()));

    for u in dns_socks {
        u.set_nonblocking(true)?;
        tokio::spawn(dns::serve(
            tokio::net::UdpSocket::from_std(u)?,
            zone.clone(),
        ));
    }
    let tls = proxy::tls_config(certs.clone());
    let limits = limits(&opts, open_files);
    let budget = Budget::new(limits);
    let proxy = Arc::new(Proxy::new(routes, Some(budget.clone())));
    let tracker = ConnTracker::default();
    for l in https {
        l.set_nonblocking(true)?;
        let l = tokio::net::TcpListener::from_std(l)?;
        tokio::spawn(proxy::serve_https(
            l,
            tls.clone(),
            proxy.clone(),
            srx.clone(),
            tracker.clone(),
            budget.clone(),
        ));
    }
    for l in http {
        l.set_nonblocking(true)?;
        let l = tokio::net::TcpListener::from_std(l)?;
        tokio::spawn(proxy::serve_http(
            l,
            proxy.clone(),
            srx.clone(),
            tracker.clone(),
            budget.clone(),
        ));
    }
    let _watcher = watch_state(&paths, core.clone());
    tracing::info!(
        "lokal {} (pid {}) serving {} name(s) on {} [{source}, {name_backend}] — ready in {}ms",
        crate::version::build_id(),
        std::process::id(),
        r.ready.len(),
        listener_desc.join(", "),
        t0.elapsed().as_millis()
    );
    tracing::info!(
        "connection limits: {open_files} open files → {} client and {} upstream connections; deadlines {}s before the first request, {}s idle",
        limits.max_clients,
        limits.max_upstream,
        limits.pre_request.as_secs_f32(),
        limits.idle.as_secs_f32()
    );

    wait_for_signal().await;
    tracing::info!("shutting down");
    let _ = stx.send(true);
    tracker.drain(SHUTDOWN_GRACE).await;
    core.names.lock().await.clear().await;
    let _ = std::fs::remove_file(&paths.socket);
    tracing::info!("stopped");
    Ok(0)
}

async fn wait_for_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("SIGTERM handler");
    let mut int = signal(SignalKind::interrupt()).expect("SIGINT handler");
    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}

/// Watches `state.toml` as a backup to IPC `reload` (hand edits).
fn watch_state(paths: &Paths, core: Arc<Core>) -> Option<notify::RecommendedWatcher> {
    use notify::Watcher;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<()>();
    let state_name = paths.state.file_name().map(|n| n.to_os_string());
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res
            && ev
                .paths
                .iter()
                .any(|p| p.file_name().map(|n| n.to_os_string()) == state_name)
        {
            let _ = tx.send(());
        }
    })
    .map_err(|e| tracing::warn!("state watcher disabled: {e}"))
    .ok()?;
    if let Err(e) = w.watch(&paths.base, notify::RecursiveMode::NonRecursive) {
        tracing::warn!("state watcher disabled: {e}");
        return None;
    }
    tokio::spawn(async move {
        while rx.recv().await.is_some() {
            tokio::time::sleep(Duration::from_millis(50)).await;
            while rx.try_recv().is_ok() {}
            core.reload().await;
        }
    });
    Some(w)
}
