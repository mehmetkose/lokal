//! Onboarding (`setup`, SPEC §4.6), `uninstall` (§4.7), `upgrade`, and
//! background-service control. Privileged steps go only through
//! `TrustStore::plan_*` and are printed before they run.

pub mod record;

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::ca::{CA_CN_PREFIX, CaCert};
use crate::cli::output::{self as out, dim, ok_mark, step, warn_mark};
use crate::dns;
use crate::error::{EXIT_SUDO_FORBIDDEN, UserError};
use crate::ipc::{self, IpcError, SelftestAction};
use crate::net::{self, Layer, Trust};
use crate::paths::{LABEL, Paths};
use crate::platform::{
    self, InstallCtx, InstallOutcome, Platform, ResolverStatus, TrustStatus, TrustStore,
};
use crate::util;
use record::InstallRecord;

pub const PING_WAIT: Duration = Duration::from_secs(5);

/// Setup has run: `install.json` and the service definition exist.
pub fn is_set_up(paths: &Paths) -> bool {
    paths.install_record.is_file() && paths.plist.as_ref().is_some_and(|p| p.is_file())
}

pub fn require_setup(paths: &Paths) -> Result<()> {
    if paths.sandbox {
        bail!(UserError::runtime(
            "managing the background service is disabled under LOKAL_HOME (sandbox)"
        ));
    }
    if !is_set_up(paths) {
        bail!(UserError::not_set_up());
    }
    Ok(())
}

pub fn install_ctx(paths: &Paths, binary_changed: bool) -> Result<InstallCtx> {
    // No service definition: a sandbox, or an OS where lokal can't install
    // the background service yet (Linux, for now).
    let Some(definition) = paths.plist.clone() else {
        bail!(UserError::runtime(if paths.sandbox {
            "not available under LOKAL_HOME"
        } else {
            platform::UNSUPPORTED
        }));
    };
    Ok(InstallCtx {
        label: LABEL.into(),
        definition,
        binary: paths.daemon_bin.clone(),
        log_dir: paths.log_dir.clone(),
        stdout_log: paths.launchd_out.clone(),
        stderr_log: paths.launchd_err.clone(),
        binary_changed,
    })
}

fn forbid_sudo() -> bool {
    std::env::var("LOKAL_FORBID_SUDO").is_ok_and(|v| v == "1")
}

/// Waits for a `ping` from a daemon running `build_id` (any build if None)
/// whose pid differs from `not_pid`.
pub fn wait_for_ping(
    paths: &Paths,
    build_id: Option<&str>,
    not_pid: Option<u32>,
    limit: Duration,
) -> Result<ipc::Ping> {
    let deadline = Instant::now() + limit;
    let mut last;
    loop {
        match ipc::ping(paths, Duration::from_millis(500)) {
            Ok(p) if build_id.is_none_or(|b| b == p.build_id) && Some(p.pid) != not_pid => {
                return Ok(p);
            }
            Ok(p) => last = format!("still running build {} (pid {})", p.build_id, p.pid),
            Err(e) => last = e.to_string(),
        }
        if Instant::now() > deadline {
            let logs = util::tail_lines(&paths.daemon_log, 5).join("\n    ");
            bail!(
                UserError::runtime(format!(
                    "the background service didn't answer within {}s ({last})",
                    limit.as_secs()
                ))
                .hint(format!(
                    "run 'lokal logs' or 'lokal doctor'. Last log lines:\n    {logs}"
                ))
            );
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// Writes `ca/bundle.pem`: the macOS system roots plus the lokal CA.
pub fn write_bundle(paths: &Paths, p: &Platform, ca: &CaCert) -> Result<()> {
    let mut pem = p.trust.system_roots_pem()?;
    if !pem.ends_with('\n') {
        pem.push('\n');
    }
    pem.push_str(&ca.pem);
    util::write_atomic(&paths.ca_bundle, pem.as_bytes(), 0o644)
}

// ───────────────────────────── selftest ─────────────────────────────

pub struct SelftestOk {
    /// The names that were checked (one per TLD).
    pub fqdns: Vec<String>,
    pub elapsed: Duration,
}

#[derive(Debug)]
pub struct SelftestFail {
    /// `daemon`, `names`, `dns`, `connect`, `tls`, `http`
    pub layer: &'static str,
    pub message: String,
}

impl SelftestFail {
    pub fn hint(&self) -> &'static str {
        match self.layer {
            "daemon" => {
                "the background service isn't answering: run 'lokal start', then 'lokal logs'"
            }
            "names" => {
                "mDNSResponder didn't register the name: check 'lokal logs' and 'log show --last 2m --predicate \"process == \\\"mDNSResponder\\\"\"'"
            }
            "dns" => "the name didn't resolve to 127.0.0.1 and ::1: run 'lokal doctor'",
            "connect" => "nothing answered on port 443: run 'lokal doctor' (listeners)",
            "tls" => {
                "macOS doesn't trust lokal's certificate: run 'lokal setup' (it will ask for your password)"
            }
            _ => "unexpected answer from lokal: run 'lokal logs'",
        }
    }
}

/// The full selftest (SPEC §4.6 step 4): the daemon publishes one throwaway
/// name per TLD; for each we resolve it, do a TLS handshake with `trust`,
/// GET it; then end the selftest. `.lokal` is skipped when `check_lokal` is
/// false (no OS resolver installed, e.g. `setup --skip-trust`).
pub fn selftest(
    paths: &Paths,
    trust: &Trust,
    check_lokal: bool,
) -> std::result::Result<SelftestOk, SelftestFail> {
    let t = Instant::now();
    let begin: ipc::Selftest = ipc::call(
        paths,
        &ipc::Request::Selftest {
            action: SelftestAction::Begin,
        },
        Duration::from_secs(5),
    )
    .map_err(|e| SelftestFail {
        layer: "daemon",
        message: e.to_string(),
    })?;
    let mut names = begin.fqdns.clone();
    if names.is_empty() {
        names.extend(begin.fqdn.clone());
    }
    let checked: Vec<String> = names
        .iter()
        .filter(|n| check_lokal || !crate::dns::in_zone(n))
        .cloned()
        .collect();
    let res = (|| {
        if !begin.ok {
            return Err(SelftestFail {
                layer: "names",
                message: begin
                    .error
                    .clone()
                    .unwrap_or_else(|| "selftest name not ready".into()),
            });
        }
        let cfg = net::client_config(trust).map_err(|e| SelftestFail {
            layer: "tls",
            message: format!("{e:#}"),
        })?;
        for fqdn in &checked {
            let (r, _) = net::resolve(fqdn);
            let addrs = r.map_err(|e| SelftestFail {
                layer: "dns",
                message: format!("{e:#}"),
            })?;
            if !net::has_both_loopbacks(&addrs) {
                return Err(SelftestFail {
                    layer: "dns",
                    message: format!("{fqdn} resolved to {addrs:?}, expected 127.0.0.1 and ::1"),
                });
            }
            let resp = net::https_get(fqdn, "/", None, cfg.clone(), Duration::from_secs(3))
                .map_err(|e| SelftestFail {
                    layer: match e.layer {
                        Layer::Dns => "dns",
                        Layer::Connect => "connect",
                        Layer::Tls => "tls",
                        Layer::Http => "http",
                    },
                    message: format!("{fqdn}: {}", e.message),
                })?;
            if resp.status != 200 || resp.body.trim() != "lokal ok" {
                return Err(SelftestFail {
                    layer: "http",
                    message: format!(
                        "{fqdn}: expected `200 lokal ok`, got {} {:?}",
                        resp.status,
                        resp.body.trim()
                    ),
                });
            }
        }
        Ok(())
    })();
    let _ = ipc::call::<ipc::Selftest>(
        paths,
        &ipc::Request::Selftest {
            action: SelftestAction::End,
        },
        Duration::from_secs(5),
    );
    res.map(|_| SelftestOk {
        fqdns: checked,
        elapsed: t.elapsed(),
    })
}

// ───────────────────────────── setup ─────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct SetupOpts {
    pub yes: bool,
    pub dry_run: bool,
    pub skip_trust: bool,
}

fn refuse_root(p: &Platform, what: &str) -> Result<()> {
    if p.system.is_root() {
        bail!(UserError::runtime(format!(
            "run 'lokal {what}' as yourself, not with sudo — lokal asks for your password only for the one step that needs it"
        )));
    }
    Ok(())
}

/// Fails (exit 4) if `cmds` need sudo but it can't run here:
/// `LOKAL_FORBID_SUDO=1` (`forbid`), or no terminal for the password (`!tty`).
fn check_sudo_possible(
    cmds: &[platform::PrivilegedCommand],
    purpose: &str,
    forbid: bool,
    tty: bool,
) -> std::result::Result<(), UserError> {
    if cmds.is_empty() {
        return Ok(());
    }
    if forbid {
        return Err(UserError::new(
            EXIT_SUDO_FORBIDDEN,
            format!("{purpose} needs sudo, which LOKAL_FORBID_SUDO=1 forbids"),
        )
        .hint(
            cmds.iter()
                .map(|c| c.display())
                .collect::<Vec<_>>()
                .join("\n      "),
        ));
    }
    if !tty {
        return Err(UserError::new(
            EXIT_SUDO_FORBIDDEN,
            format!("{purpose} needs your password, but this isn't an interactive terminal"),
        )
        .hint("ask the user to run `lokal setup` in their own Terminal (it asks for their password once)"));
    }
    Ok(())
}

/// Runs the privileged commands (one sudo prompt; sudo caches it).
fn run_privileged(p: &Platform, cmds: &[platform::PrivilegedCommand], purpose: &str) -> Result<()> {
    check_sudo_possible(cmds, purpose, forbid_sudo(), util::stdin_is_tty())?;
    let user = p.system.user_and_host().0;
    let prompt = format!("Password for {user} (to {purpose}): ");
    for c in cmds {
        p.system.run_privileged(c, &prompt)?;
    }
    Ok(())
}

/// What setup must do about trusting the CA it keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrustNeed {
    /// Trusted for TLS only: nothing to do.
    Done,
    /// Not trusted, or there's no usable CA yet: trust it.
    Trust,
    /// Trusted, but for every use (an older lokal ran `add-trusted-cert`
    /// without `-p ssl`): trusting it again narrows it (SPEC §6.3).
    Narrow,
}

impl TrustNeed {
    /// Does it take `plan_trust`'s command, and so the user's password?
    fn needs_privilege(self) -> bool {
        self != TrustNeed::Done
    }
}

fn trust_need(trust: &dyn TrustStore, ca: Option<&CaCert>) -> Result<TrustNeed> {
    let Some(ca) = ca else {
        return Ok(TrustNeed::Trust);
    };
    Ok(match trust.is_trusted(ca)? {
        TrustStatus::NotTrusted(_) => TrustNeed::Trust,
        TrustStatus::Trusted if trust.trust_too_broad(ca)? => TrustNeed::Narrow,
        TrustStatus::Trusted => TrustNeed::Done,
    })
}

/// Is `ca` in the System keychain (trusted or merely present)?
fn in_keychain(trust: &dyn TrustStore, ca: &CaCert) -> bool {
    trust.is_present(ca).unwrap_or(false)
        || trust
            .is_trusted(ca)
            .is_ok_and(|s| s == TrustStatus::Trusted)
}

/// The CA in `root.pem`: `(with its key, without it)`. A certificate whose
/// key file is gone can't sign leaves, but it may still be trusted, so setup
/// retires it rather than writing a new CA over it.
fn survey_ca(paths: &Paths) -> Result<(Option<CaCert>, Option<CaCert>)> {
    if CaCert::exists(paths) {
        return Ok((Some(CaCert::load(paths)?), None));
    }
    Ok((None, CaCert::load_cert_only(&paths.ca_cert).ok()))
}

/// lokal CAs that setup or uninstall must untrust.
#[derive(Default)]
struct OldCas {
    /// In the keychain: untrust and delete each of these.
    cas: Vec<CaCert>,
    /// install.json's CA, found only in the keychain because its files are
    /// gone. Also in `cas`, planned at its retired path: write it there with
    /// `write_cert` before the privileged batch runs.
    recovered: Option<CaCert>,
    /// Looking up install.json's CA in the keychain failed.
    lookup_error: Option<String>,
}

/// Every lokal CA the keychain still has, except `keep` (the CA setup goes on
/// using). Chosen by certificate, not certificate plus key: a CA whose key
/// file is gone is still trusted (SPEC §4.7). With `retire_current`, a
/// `root.pem` that isn't kept is planned at its retired path, because setup
/// moves it there before the batch runs.
fn old_cas(
    paths: &Paths,
    trust: &dyn TrustStore,
    keep: Option<&CaCert>,
    retire_current: bool,
) -> OldCas {
    let mut on_disk = CaCert::load_retired(paths);
    if let Ok(c) = CaCert::load_cert_only(&paths.ca_cert) {
        on_disk.push(if retire_current {
            c.with_path(c.retired_path(paths))
        } else {
            c
        });
    }
    let mut old = OldCas::default();
    for c in &on_disk {
        let kept = keep.is_some_and(|k| k.sha1 == c.sha1);
        if !kept && !old.cas.iter().any(|x| x.sha1 == c.sha1) && in_keychain(trust, c) {
            old.cas.push(c.clone());
        }
    }
    match recorded_ca_only_in_keychain(paths, trust, &on_disk) {
        Ok(Some(c)) => {
            old.cas.push(c.clone());
            old.recovered = Some(c);
        }
        Ok(None) => {}
        Err(e) => old.lookup_error = Some(format!("{e:#}")),
    }
    old
}

/// The CA install.json records, when no certificate on disk has it but the
/// keychain does: the CA files were deleted while it was still trusted.
/// Matched on the SHA-1 lokal computes from each lokal CA in the keychain;
/// install.json is user-writable, so its hash alone never reaches a
/// privileged command.
fn recorded_ca_only_in_keychain(
    paths: &Paths,
    trust: &dyn TrustStore,
    on_disk: &[CaCert],
) -> Result<Option<CaCert>> {
    let Some(want) = InstallRecord::load(paths)
        .map(|r| r.ca_sha1)
        .filter(|h| !h.is_empty())
    else {
        return Ok(None);
    };
    if on_disk.iter().any(|c| c.sha1.eq_ignore_ascii_case(&want)) {
        return Ok(None);
    }
    let found = CaCert::all_from_pem(&trust.lokal_cas_pem()?)
        .into_iter()
        .find(|c| c.sha1.eq_ignore_ascii_case(&want) && c.common_name().starts_with(CA_CN_PREFIX));
    Ok(found.map(|c| {
        let path = c.retired_path(paths);
        c.with_path(path)
    }))
}

/// Stages the resolver config in the user's data dir; returns its path.
fn stage_resolver(paths: &Paths, p: &Platform) -> Result<std::path::PathBuf> {
    let staged = paths.resolver_staged(dns::ZONE);
    util::write_atomic(
        &staged,
        p.resolver.config(dns::DEFAULT_PORT).as_bytes(),
        0o644,
    )?;
    Ok(staged)
}

/// Waits (bounded) until the OS uses the resolver config. Setup only.
fn wait_resolver_active(p: &Platform, limit: Duration) -> ResolverStatus {
    let deadline = Instant::now() + limit;
    loop {
        let st = p.resolver.status(dns::ZONE, dns::DEFAULT_PORT);
        if st == ResolverStatus::Installed || Instant::now() > deadline {
            return st;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn setup(opts: SetupOpts) -> Result<i32> {
    let p = platform::current();
    refuse_root(&p, "setup")?;
    let paths = Paths::resolve()?;
    if paths.sandbox {
        bail!(UserError::runtime(
            "setup is not available under LOKAL_HOME (sandbox)"
        ));
    }
    let (user, host) = p.system.user_and_host();

    // Survey what's already in place.
    let (ca_existing, ca_keyless) = survey_ca(&paths)?;
    // A CA whose name constraints aren't exactly .local + .lokal (an older
    // lokal's CA lacks .lokal, or also allows .test and .internal).
    let ca_outdated = ca_existing.as_ref().is_some_and(|c| !c.is_current());
    let ca_usable = ca_existing.clone().filter(|c| c.is_current());
    // Old CAs still in the keychain, each planned against where it will live
    // once retired: retired ones, the current one if it's outdated or its key
    // is gone, and install.json's CA if only the keychain still has it.
    let OldCas {
        cas: old_in_keychain,
        recovered,
        lookup_error,
    } = if opts.skip_trust {
        OldCas::default()
    } else {
        old_cas(&paths, p.trust.as_ref(), ca_usable.as_ref(), true)
    };
    let trust_need = if opts.skip_trust {
        None
    } else {
        Some(trust_need(p.trust.as_ref(), ca_usable.as_ref())?)
    };
    let trusted_now = matches!(trust_need, Some(TrustNeed::Done | TrustNeed::Narrow));
    // Trusted for every use: trusting it again limits it to TLS.
    let narrow = trust_need == Some(TrustNeed::Narrow);
    let resolver_now = p.resolver.status(dns::ZONE, dns::DEFAULT_PORT);
    let exe = std::env::current_exe().context("cannot find the lokal executable")?;
    let ctx0 = install_ctx(&paths, false)?;
    let plist_same = p.service.definition_current(&ctx0);
    let bin_same = std::fs::read(&exe).ok() == std::fs::read(&paths.daemon_bin).ok();
    let daemon_ok = plist_same
        && bin_same
        && !ca_outdated
        && p.service.is_loaded()
        && ipc::ping(&paths, Duration::from_millis(500))
            .is_ok_and(|x| x.build_id == crate::version::build_id());

    let need_trust = trust_need.is_some_and(TrustNeed::needs_privilege);
    let need_resolver = !opts.skip_trust && resolver_now != ResolverStatus::Installed;
    // The whole privileged batch, planned up front (a preview CA stands in
    // for one that doesn't exist yet; same path).
    let new_ca_for_plan = match &ca_usable {
        Some(c) => c.clone(),
        None => CaCert::preview(&paths, &user, &host)?,
    };
    let mut plan: Vec<platform::PrivilegedCommand> = vec![];
    for old in &old_in_keychain {
        plan.extend(p.trust.plan_untrust(old));
    }
    if need_trust {
        plan.extend(p.trust.plan_trust(&new_ca_for_plan));
    }
    if need_resolver {
        plan.extend(
            p.resolver
                .plan_install(dns::ZONE, &paths.resolver_staged(dns::ZONE)),
        );
    }

    let done = |b: bool| {
        if b {
            format!("  {}", dim("(already done)"))
        } else {
            String::new()
        }
    };
    println!(
        "Welcome to lokal. This one-time setup makes https://<anything>.lokal (and .local) work on this Mac.\n"
    );
    if let Some(why) = ca_existing
        .as_ref()
        .filter(|_| ca_outdated)
        .and_then(|c| c.scope_problem())
    {
        println!("  1. Replace lokal's certificate authority ({why})");
    } else if ca_keyless.is_some() {
        println!("  1. Replace lokal's certificate authority (its private key is missing)");
    } else {
        println!(
            "  1. Create lokal's certificate authority{}",
            done(ca_existing.is_some())
        );
    }
    println!("     Only valid for .lokal and .local names. Its key stays in your user folder.");
    if opts.skip_trust {
        println!(
            "  2. Trust it in the macOS System keychain                       {}",
            dim("(skipped: --skip-trust)")
        );
        println!(
            "  3. Send .lokal lookups to lokal ({})          {}",
            p.resolver.location(dns::ZONE),
            dim("(skipped: --skip-trust)")
        );
    } else {
        if need_trust || !old_in_keychain.is_empty() {
            let what = if !old_in_keychain.is_empty() {
                "Trust it in the System keychain (and remove the old one)"
            } else if narrow {
                "Trust it for HTTPS only (it's trusted for every use now)"
            } else {
                "Trust it in the macOS System keychain"
            };
            println!("  2. {what:<60} ← needs your password (once)");
        } else {
            println!("  2. Trust it in the macOS System keychain{}", done(true));
        }
        if need_resolver {
            println!(
                "  3. Send .lokal lookups to lokal ({})          ← same password",
                p.resolver.location(dns::ZONE)
            );
        } else {
            println!(
                "  3. Send .lokal lookups to lokal ({}){}",
                p.resolver.location(dns::ZONE),
                done(true)
            );
        }
        if !plan.is_empty() {
            println!(
                "     You type your password once in Terminal (sudo); macOS then shows its own dialog for the keychain."
            );
        }
    }
    println!(
        "  4. Install the background service (LaunchAgent, runs as you){}",
        done(daemon_ok)
    );
    println!(
        "     macOS will hold ports 443 and 80 (and DNS on {}) on 127.0.0.1 for it. Nothing is reachable from your network.",
        dns::DEFAULT_PORT
    );
    println!("  5. Run a self-test\n");
    if let Some(e) = &lookup_error {
        println!(
            "{} couldn't look for the CA install.json records in the keychain: {e}\n",
            warn_mark()
        );
    }

    if opts.dry_run {
        if plan.is_empty() {
            println!("No privileged command is needed.");
        } else {
            println!("Privileged commands (the only ones; run with sudo):");
            for c in &plan {
                println!("  {}", c.display());
            }
        }
        println!("\nDry run: nothing was changed.");
        return Ok(0);
    }

    if !opts.yes {
        if !util::stdin_is_tty() {
            bail!(
                UserError::usage("setup needs confirmation").hint(
                    "run it in a Terminal (it may ask for the user's password), or pass --yes"
                )
            );
        }
        if !out::confirm("Continue?", true)? {
            println!("Nothing was changed.");
            return Ok(1);
        }
        println!();
    }

    // Fail before changing anything if the privileged batch can't run:
    // replacing the CA deletes the old key, and an untrusted new CA would
    // break every lokal URL until the user runs setup with their password.
    check_sudo_possible(
        &plan,
        "set up lokal's certificate and .lokal names",
        forbid_sudo(),
        util::stdin_is_tty(),
    )?;

    let mut changed = false;
    // 1. CA
    let t = Instant::now();
    let mut ca_changed = false;
    let ca = match (ca_existing, ca_outdated) {
        (Some(ca), false) => {
            step(&ok_mark(), "Certificate authority already exists", None);
            ca
        }
        (Some(old), true) => {
            old.retire(&paths)?;
            let ca = CaCert::create(&paths, &user, &host)?;
            changed = true;
            ca_changed = true;
            step(
                &ok_mark(),
                "Replaced the certificate authority (now valid for .lokal and .local only)",
                Some(t.elapsed()),
            );
            ca
        }
        (None, _) => {
            // Never write over a certificate that may still be trusted.
            let replaced = match ca_keyless {
                Some(old) => {
                    old.retire(&paths)?;
                    true
                }
                None => false,
            };
            let ca = CaCert::create(&paths, &user, &host)?;
            changed = true;
            ca_changed = true;
            step(
                &ok_mark(),
                if replaced {
                    "Replaced the certificate authority (the old one's key was missing)"
                } else {
                    "Created certificate authority"
                },
                Some(t.elapsed()),
            );
            ca
        }
    };

    // 2 + 3. One privileged batch: untrust old CAs, trust this one, resolver.
    let mut trusted = trusted_now;
    let mut resolver_installed = resolver_now == ResolverStatus::Installed;
    if opts.skip_trust {
        step(
            &warn_mark(),
            "Trust and resolver: skipped (--skip-trust, test only)",
            None,
        );
    } else {
        if let Some(c) = &recovered {
            c.write_cert()?;
        }
        let mut cmds: Vec<platform::PrivilegedCommand> = vec![];
        for old in &old_in_keychain {
            cmds.extend(p.trust.plan_untrust(old));
        }
        if need_trust {
            cmds.extend(p.trust.plan_trust(&ca));
        }
        if need_resolver {
            let staged = stage_resolver(&paths, &p)?;
            cmds.extend(p.resolver.plan_install(dns::ZONE, &staged));
        }
        if cmds.is_empty() {
            step(&ok_mark(), "Already trusted", None);
            step(&ok_mark(), ".lokal lookups already go to lokal", None);
        } else {
            let t = Instant::now();
            println!(
                "  › Changing system settings. Enter your password here, then again in the macOS dialog."
            );
            for c in &cmds {
                println!("    {}", c.display());
            }
            run_privileged(&p, &cmds, "set up lokal's certificate and .lokal names")?;
            changed = true;
            if !old_in_keychain.is_empty() {
                step(
                    &ok_mark(),
                    "Removed the old certificate authority from the keychain",
                    None,
                );
            }
            match p.trust.is_trusted(&ca)? {
                TrustStatus::Trusted => {}
                TrustStatus::NotTrusted(why) => bail!(
                    UserError::runtime(format!(
                        "macOS still doesn't trust lokal's certificate after adding it: {why}"
                    ))
                    .hint("run 'lokal doctor'")
                ),
            }
            if p.trust.trust_too_broad(&ca)? {
                // Another trust setting, outside the System keychain's, still
                // covers every use; asking again would change nothing.
                bail!(
                    UserError::runtime(
                        "macOS still trusts lokal's certificate for more than HTTPS, because of another trust setting for it (such as \"Always Trust\" in Keychain Access)"
                    )
                    .hint("in Keychain Access, open \"lokal development CA\", set Trust to \"Use System Defaults\", then run 'lokal setup' again")
                );
            }
            trusted = true;
            step(&ok_mark(), "Trusted in System keychain", Some(t.elapsed()));
            if need_resolver {
                match wait_resolver_active(&p, Duration::from_secs(5)) {
                    ResolverStatus::Installed => {
                        resolver_installed = true;
                        step(
                            &ok_mark(),
                            &format!(
                                "Installed {} (.lokal → lokal)",
                                p.resolver.location(dns::ZONE)
                            ),
                            None,
                        );
                    }
                    other => bail!(
                        UserError::runtime(format!(
                            "the .lokal resolver isn't active after installing it: {other:?}"
                        ))
                        .hint("run 'lokal doctor'")
                    ),
                }
            }
        }
        // Retired CAs that are out of the keychain are no longer needed.
        for c in CaCert::load_retired(&paths) {
            if !in_keychain(p.trust.as_ref(), &c)
                && let Some(dir) = c.path.parent()
            {
                let _ = std::fs::remove_dir_all(dir);
            }
        }
        let _ = std::fs::remove_dir(&paths.ca_retired_dir);
    }

    // 4. Background service (restarted when the CA changed: it loads the CA at start)
    let t = Instant::now();
    util::create_private_dir(&paths.bin_dir)?;
    let bin_changed = util::copy_atomic(&exe, &paths.daemon_bin, 0o755)?;
    let ctx = install_ctx(&paths, bin_changed || ca_changed)?;
    let old_pid = ipc::ping(&paths, Duration::from_millis(300))
        .ok()
        .map(|x| x.pid);
    let outcome = p.service.install(&ctx)?;
    let not_pid = if outcome == InstallOutcome::Unchanged {
        None
    } else {
        old_pid
    };
    let ping = wait_for_ping(&paths, Some(crate::version::build_id()), not_pid, PING_WAIT)?;
    let status: ipc::Status = ipc::call(&paths, &ipc::Request::Status, Duration::from_secs(2))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let listeners = status.listeners.join(", ");
    match outcome {
        InstallOutcome::Unchanged => step(
            &ok_mark(),
            &format!("Background service already running ({listeners})"),
            None,
        ),
        _ => {
            changed = true;
            step(
                &ok_mark(),
                &format!("Background service running ({listeners})"),
                Some(t.elapsed()),
            );
        }
    }
    out::debug(format!("daemon pid {} build {}", ping.pid, ping.build_id));

    // Firefox and other per-user extras.
    let mut extras = vec![];
    if trusted {
        match p.trust.apply_user_extras(&ca) {
            Ok(done) => {
                for d in &done {
                    step(&ok_mark(), d, None);
                    changed = true;
                }
                extras = done;
            }
            Err(e) => step(&warn_mark(), &format!("Firefox: {e:#}"), None),
        }
    }

    // 5. Self-test
    let trust = if opts.skip_trust {
        Trust::CaFile(paths.ca_cert.clone())
    } else {
        Trust::Platform
    };
    match selftest(&paths, &trust, resolver_installed) {
        Ok(okk) => {
            let by = if opts.skip_trust {
                "verified with the CA file (trust: skipped)"
            } else {
                "verified by macOS"
            };
            let names: Vec<String> = okk.fqdns.iter().map(|f| format!("https://{f}")).collect();
            step(
                &ok_mark(),
                &format!("Self-test: {} {by}", names.join(" and ")),
                Some(okk.elapsed),
            );
        }
        Err(f) => {
            step(
                &out::fail_mark(),
                &format!("Self-test failed at the {} layer: {}", f.layer, f.message),
                None,
            );
            bail!(UserError::runtime(format!("self-test failed ({})", f.layer)).hint(f.hint()));
        }
    }

    // 6. Record + bundle
    let mut rec = InstallRecord::load(&paths).unwrap_or_default();
    let prev_extras = std::mem::take(&mut rec.user_extras);
    rec = InstallRecord {
        schema: 1,
        version: crate::version::VERSION.into(),
        build_id: crate::version::build_id().into(),
        installed_at: if rec.installed_at.is_empty() {
            util::now_rfc3339()
        } else {
            rec.installed_at
        },
        label: LABEL.into(),
        plist: ctx.definition.clone(),
        daemon_binary: paths.daemon_bin.clone(),
        ca_cert: paths.ca_cert.clone(),
        ca_sha256: ca.sha256.clone(),
        ca_sha1: ca.sha1.clone(),
        trusted,
        trust_skipped: opts.skip_trust,
        resolver_installed,
        user_extras: if extras.is_empty() {
            prev_extras
        } else {
            extras
        },
    };
    rec.save(&paths)?;
    if let Err(e) = write_bundle(&paths, &p, &ca) {
        step(
            &warn_mark(),
            &format!("Could not write bundle.pem: {e:#}"),
            None,
        );
    }

    println!();
    if !changed {
        println!("Everything was already set up — nothing to change.");
    } else {
        println!("You're all set. Next:");
        println!("  lokal project create myproject");
        println!("  lokal service add api 4000");
        println!("Tip: run 'eval \"$(lokal env)\"' so Node and Python trust lokal too.");
    }
    Ok(0)
}

// ───────────────────────────── uninstall ─────────────────────────────

pub fn uninstall(purge: bool, yes: bool) -> Result<i32> {
    let p = platform::current();
    refuse_root(&p, "uninstall")?;
    let paths = Paths::resolve()?;
    if paths.sandbox {
        bail!(UserError::runtime(
            "uninstall is not available under LOKAL_HOME (sandbox)"
        ));
    }
    let ctx = install_ctx(&paths, false)?;
    // Every lokal CA still in the keychain: the current one (with or without
    // its key), retired ones, and install.json's CA if only the keychain has it.
    let OldCas {
        cas,
        recovered,
        lookup_error,
    } = old_cas(&paths, p.trust.as_ref(), None, false);
    let mut untrust_cmds = vec![];
    for c in &cas {
        untrust_cmds.extend(p.trust.plan_untrust(c));
    }
    let resolver_present =
        p.resolver.status(dns::ZONE, dns::DEFAULT_PORT) != ResolverStatus::Missing;
    let resolver_cmds = if resolver_present {
        p.resolver.plan_uninstall(dns::ZONE)
    } else {
        vec![]
    };

    println!("This removes lokal from this Mac:\n");
    println!("  1. Stop and remove the background service (LaunchAgent, its copy of lokal)");
    if let (true, Some(e)) = (untrust_cmds.is_empty(), &lookup_error) {
        println!(
            "  2. Untrust the certificate authority                          {}",
            out::yellow(&format!(
                "(couldn't look for the CA install.json records in the keychain: {e})"
            ))
        );
    } else if untrust_cmds.is_empty() {
        println!(
            "  2. Untrust the certificate authority                          {}",
            dim("(not in the keychain)")
        );
    } else {
        println!(
            "  2. Untrust and remove the certificate authority               ← needs your password"
        );
    }
    if resolver_cmds.is_empty() {
        println!(
            "  3. Remove {:<55}{}",
            p.resolver.location(dns::ZONE),
            dim("(not installed)")
        );
    } else {
        println!(
            "  3. Remove {} (.lokal lookups)                ← same password",
            p.resolver.location(dns::ZONE)
        );
    }
    if !untrust_cmds.is_empty() || !resolver_cmds.is_empty() {
        println!("     (in Terminal for sudo; macOS may show its own dialog asking for it again)");
        for c in untrust_cmds.iter().chain(resolver_cmds.iter()) {
            println!("       {}", c.display());
        }
    }
    println!("  4. Remove lokal's Firefox setting (if it added one)");
    if purge {
        println!(
            "  5. Delete all lokal data: projects, the CA, logs ({})",
            paths.base.display()
        );
    } else {
        println!(
            "\n  Your projects (state.toml) and the CA files are kept; 'lokal setup' restores everything."
        );
    }
    println!();
    if !yes {
        if !util::stdin_is_tty() {
            bail!(
                UserError::usage("uninstall needs confirmation").hint(
                    "run it in a Terminal (it may ask for the user's password), or pass --yes"
                )
            );
        }
        if !out::confirm("Continue?", false)? {
            println!("Nothing was changed.");
            return Ok(1);
        }
    }
    // A CA found only in the keychain needs its certificate on disk for
    // `remove-trusted-cert`. Written before anything changes.
    if let Some(c) = &recovered {
        c.write_cert()?;
    }

    let mut warnings = 0;
    // 1. service
    match p.service.uninstall(&ctx) {
        Ok(()) => step(&ok_mark(), "Background service removed", None),
        Err(e) => {
            warnings += 1;
            step(&warn_mark(), &format!("Background service: {e:#}"), None);
        }
    }
    match std::fs::remove_file(&paths.daemon_bin) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            warnings += 1;
            step(
                &warn_mark(),
                &format!("Could not remove {}: {e}", paths.daemon_bin.display()),
                None,
            );
        }
    }
    let _ = std::fs::remove_dir(&paths.bin_dir);
    let _ = std::fs::remove_file(&paths.socket);

    // 2 + 3. trust and resolver, one privileged batch
    let mut cmds = untrust_cmds.clone();
    cmds.extend(resolver_cmds.iter().cloned());
    if !cmds.is_empty() {
        run_privileged(&p, &cmds, "remove lokal's certificate and .lokal resolver")?;
    }
    if let (true, Some(e)) = (untrust_cmds.is_empty(), &lookup_error) {
        warnings += 1;
        step(
            &warn_mark(),
            &format!("Couldn't look for the CA install.json records in the keychain: {e}"),
            None,
        );
    } else if untrust_cmds.is_empty() {
        step(
            &ok_mark(),
            "Certificate authority not in the keychain",
            None,
        );
    } else {
        step(
            &ok_mark(),
            "Certificate authority untrusted and removed from the keychain",
            None,
        );
    }
    let _ = std::fs::remove_dir_all(&paths.ca_retired_dir);
    if resolver_cmds.is_empty() {
        step(
            &ok_mark(),
            &format!("{} not installed", p.resolver.location(dns::ZONE)),
            None,
        );
    } else {
        step(
            &ok_mark(),
            &format!("Removed {}", p.resolver.location(dns::ZONE)),
            None,
        );
    }
    let _ = std::fs::remove_dir_all(&paths.resolver_dir);

    // 4. extras
    match p.trust.remove_user_extras() {
        Ok(done) => {
            for d in done {
                step(&ok_mark(), &d, None);
            }
        }
        Err(e) => {
            warnings += 1;
            step(&warn_mark(), &format!("Firefox: {e:#}"), None);
        }
    }
    // `lokal env` points tools at bundle.pem as a trust anchor. It stops
    // exporting once install.json is gone; the bundle goes with it.
    let _ = std::fs::remove_file(&paths.ca_bundle);
    let _ = std::fs::remove_file(&paths.install_record);

    if purge {
        for dir in [&paths.base, &paths.log_dir] {
            match std::fs::remove_dir_all(dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    warnings += 1;
                    step(
                        &warn_mark(),
                        &format!("Could not delete {}: {e}", dir.display()),
                        None,
                    );
                }
            }
        }
        step(&ok_mark(), "Deleted all lokal data", None);
    }
    println!();
    if warnings > 0 {
        println!("lokal is uninstalled ({warnings} warning(s) above).");
    } else {
        println!("lokal is uninstalled.");
    }
    println!(
        "If your shell profile runs 'eval \"$(lokal env)\"' (or 'lokal env --shell fish | source'), remove that line."
    );
    Ok(0)
}

// ───────────────────────────── service control ─────────────────────────────

pub fn upgrade() -> Result<i32> {
    let paths = Paths::resolve()?;
    require_setup(&paths)?;
    let p = platform::current();
    let t = Instant::now();
    let exe = std::env::current_exe().context("cannot find the lokal executable")?;
    util::create_private_dir(&paths.bin_dir)?;
    util::copy_atomic(&exe, &paths.daemon_bin, 0o755)?;
    let ctx = install_ctx(&paths, true)?;
    let old_pid = ipc::ping(&paths, Duration::from_millis(300))
        .ok()
        .map(|x| x.pid);
    let plist_same = p.service.definition_current(&ctx);
    if plist_same && p.service.is_loaded() {
        p.service.restart()?;
    } else {
        p.service.install(&ctx)?;
    }
    wait_for_ping(&paths, Some(crate::version::build_id()), old_pid, PING_WAIT)?;
    if let Some(mut rec) = InstallRecord::load(&paths) {
        rec.version = crate::version::VERSION.into();
        rec.build_id = crate::version::build_id().into();
        let _ = rec.save(&paths);
    }
    println!(
        "{} background service now running lokal {} — ready in {}",
        ok_mark(),
        crate::version::short_version(),
        util::fmt_duration(t.elapsed())
    );
    Ok(0)
}

pub fn start() -> Result<i32> {
    let paths = Paths::resolve()?;
    require_setup(&paths)?;
    let p = platform::current();
    let t = Instant::now();
    if let Ok(ping) = ipc::ping(&paths, Duration::from_millis(300)) {
        println!(
            "{} background service is already running (pid {})",
            ok_mark(),
            ping.pid
        );
        return Ok(0);
    }
    p.service.start(&install_ctx(&paths, false)?)?;
    let ping = wait_for_ping(&paths, None, None, PING_WAIT)?;
    println!(
        "{} background service started (pid {}) — ready in {}",
        ok_mark(),
        ping.pid,
        util::fmt_duration(t.elapsed())
    );
    warn_build(&ping);
    Ok(0)
}

pub fn stop() -> Result<i32> {
    let paths = Paths::resolve()?;
    require_setup(&paths)?;
    let p = platform::current();
    p.service.stop()?;
    // bootout returns after the process is gone; confirm the socket is dead.
    let deadline = Instant::now() + Duration::from_secs(3);
    while ipc::ping(&paths, Duration::from_millis(200)).is_ok() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    println!(
        "{} background service stopped (it starts again at login, or with 'lokal start')",
        ok_mark()
    );
    Ok(0)
}

pub fn restart() -> Result<i32> {
    let paths = Paths::resolve()?;
    require_setup(&paths)?;
    let p = platform::current();
    let t = Instant::now();
    let old = ipc::ping(&paths, Duration::from_millis(300))
        .ok()
        .map(|x| x.pid);
    if p.service.is_loaded() {
        p.service.restart()?;
    } else {
        p.service.start(&install_ctx(&paths, false)?)?;
    }
    let ping = wait_for_ping(&paths, None, old, PING_WAIT)?;
    println!(
        "{} background service restarted (pid {}) — ready in {}",
        ok_mark(),
        ping.pid,
        util::fmt_duration(t.elapsed())
    );
    warn_build(&ping);
    Ok(0)
}

pub fn warn_build(ping: &ipc::Ping) {
    if ping.build_id != crate::version::build_id() {
        eprintln!(
            "{} the background service runs a different lokal build — run 'lokal upgrade'",
            out::yellow("warning:")
        );
    }
}

/// For commands that talk to the daemon: is it reachable?
pub fn daemon_state(paths: &Paths) -> DaemonState {
    match ipc::ping(paths, Duration::from_millis(1000)) {
        Ok(p) => DaemonState::Running(p),
        Err(IpcError::NotRunning) if paths.sandbox || is_set_up(paths) => DaemonState::NotRunning,
        Err(IpcError::NotRunning) => DaemonState::NotSetUp,
        Err(IpcError::Other(e)) => DaemonState::Broken(format!("{e:#}")),
    }
}

pub enum DaemonState {
    Running(ipc::Ping),
    NotRunning,
    NotSetUp,
    Broken(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd() -> Vec<platform::PrivilegedCommand> {
        vec![platform::PrivilegedCommand {
            program: "/bin/true".into(),
            args: vec!["add-trusted-cert".into()],
        }]
    }

    /// Without a service definition, lokal is never set up, and setup,
    /// uninstall and the service commands stop at `install_ctx`: under
    /// `LOKAL_HOME` with that, and where the OS has no background service
    /// yet (Linux) with "supports macOS only", not a word about a sandbox.
    #[test]
    fn no_service_definition_is_never_set_up_and_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = Paths::sandbox(dir.path().to_path_buf());
        std::fs::write(&paths.install_record, "{}").unwrap();
        assert!(!is_set_up(&paths));
        let err = |p: &Paths| install_ctx(p, false).unwrap_err().to_string();
        assert_eq!(err(&paths), "not available under LOKAL_HOME");
        paths.sandbox = false;
        assert!(!is_set_up(&paths));
        assert_eq!(err(&paths), platform::UNSUPPORTED);
    }

    /// Setup checks this before it touches the CA: with sudo forbidden (the
    /// live test's L01) or no terminal, an outdated CA must stay in place.
    #[test]
    fn sudo_precheck() {
        assert!(check_sudo_possible(&[], "x", true, false).is_ok());
        assert!(check_sudo_possible(&cmd(), "x", false, true).is_ok());
        let forbidden = check_sudo_possible(&cmd(), "x", true, true).unwrap_err();
        assert_eq!(forbidden.code, EXIT_SUDO_FORBIDDEN);
        assert!(forbidden.hint.unwrap().contains("sudo /bin/true"));
        let no_tty = check_sudo_possible(&cmd(), "x", false, false).unwrap_err();
        assert_eq!(no_tty.code, EXIT_SUDO_FORBIDDEN);
    }

    /// A System keychain that holds exactly these CAs, and answers the
    /// trust checks for the CA setup keeps as told.
    struct FakeKeychain {
        cas: Vec<CaCert>,
        trusted: bool,
        too_broad: bool,
    }

    impl FakeKeychain {
        fn holding(cas: Vec<CaCert>) -> Self {
            Self {
                cas,
                trusted: false,
                too_broad: false,
            }
        }

        fn trusting(too_broad: bool) -> Self {
            Self {
                cas: vec![],
                trusted: true,
                too_broad,
            }
        }
    }

    impl TrustStore for FakeKeychain {
        fn plan_trust(&self, _ca: &CaCert) -> Vec<platform::PrivilegedCommand> {
            vec![]
        }
        fn plan_untrust(&self, ca: &CaCert) -> Vec<platform::PrivilegedCommand> {
            vec![platform::PrivilegedCommand {
                program: "untrust".into(),
                args: vec![ca.path.display().to_string(), ca.sha1.clone()],
            }]
        }
        fn is_trusted(&self, _ca: &CaCert) -> Result<TrustStatus> {
            Ok(if self.trusted {
                TrustStatus::Trusted
            } else {
                TrustStatus::NotTrusted("fake".into())
            })
        }
        fn trust_too_broad(&self, _ca: &CaCert) -> Result<bool> {
            Ok(self.too_broad)
        }
        fn is_present(&self, ca: &CaCert) -> Result<bool> {
            Ok(self.cas.iter().any(|c| c.sha1 == ca.sha1))
        }
        fn lokal_cas_pem(&self) -> Result<String> {
            Ok(self.cas.iter().map(|c| c.pem.as_str()).collect())
        }
        fn apply_user_extras(&self, _ca: &CaCert) -> Result<Vec<String>> {
            unreachable!()
        }
        fn remove_user_extras(&self) -> Result<Vec<String>> {
            unreachable!()
        }
        fn user_extras_status(&self) -> Option<(bool, String)> {
            unreachable!()
        }
        fn system_roots_pem(&self) -> Result<String> {
            unreachable!()
        }
    }

    /// A sandbox with a CA that install.json records as trusted.
    fn installed() -> (tempfile::TempDir, Paths, CaCert) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::sandbox(dir.path().to_path_buf());
        let ca = CaCert::create(&paths, "u", "h").unwrap();
        InstallRecord {
            ca_sha1: ca.sha1.clone(),
            trusted: true,
            ..Default::default()
        }
        .save(&paths)
        .unwrap();
        (dir, paths, ca)
    }

    fn sha1s(cas: &[CaCert]) -> Vec<String> {
        cas.iter().map(|c| c.sha1.clone()).collect()
    }

    /// Deleting the key file (say, after a leak) must not hide a trusted CA
    /// from uninstall: untrusting needs only the certificate.
    #[test]
    fn uninstall_untrusts_a_ca_whose_key_is_gone() {
        let (_dir, paths, ca) = installed();
        std::fs::remove_file(&paths.ca_key).unwrap();
        let old = old_cas(
            &paths,
            &FakeKeychain::holding(vec![ca.clone()]),
            None,
            false,
        );
        assert_eq!(sha1s(&old.cas), vec![ca.sha1.clone()]);
        assert_eq!(old.cas[0].path, paths.ca_cert);
        assert!(old.recovered.is_none() && old.lookup_error.is_none());
        let untrust: Vec<_> = old
            .cas
            .iter()
            .flat_map(|c| FakeKeychain::holding(vec![]).plan_untrust(c))
            .collect();
        assert_eq!(untrust.len(), 1);
        // Not in the keychain: nothing to untrust.
        assert!(
            old_cas(&paths, &FakeKeychain::holding(vec![]), None, false)
                .cas
                .is_empty()
        );
    }

    /// With the CA files gone, uninstall finds install.json's CA in the
    /// keychain by the SHA-1 it computes, and only that one.
    #[test]
    fn uninstall_finds_the_recorded_ca_in_the_keychain_when_its_files_are_gone() {
        let (_dir, paths, ca) = installed();
        std::fs::remove_dir_all(&paths.ca_dir).unwrap();
        // Another account's lokal CA in the same System keychain.
        let other = CaCert::preview(&paths, "someone-else", "h").unwrap();
        // A hash in install.json that the keychain doesn't hold plans nothing.
        let old = old_cas(
            &paths,
            &FakeKeychain::holding(vec![other.clone()]),
            None,
            false,
        );
        assert!(old.cas.is_empty() && old.recovered.is_none());

        let old = old_cas(
            &paths,
            &FakeKeychain::holding(vec![other, ca.clone()]),
            None,
            false,
        );
        assert_eq!(sha1s(&old.cas), vec![ca.sha1.clone()]);
        let rec = old.recovered.unwrap();
        assert_eq!(rec.path, ca.retired_path(&paths));
        rec.write_cert().unwrap();
        assert_eq!(CaCert::load_cert_only(&rec.path).unwrap().sha1, ca.sha1);
    }

    /// setup must retire a `root.pem` whose key is gone (and untrust it in
    /// the same batch), not write a new CA over it.
    #[test]
    fn setup_retires_a_root_pem_whose_key_is_gone() {
        let (_dir, paths, ca) = installed();
        std::fs::remove_file(&paths.ca_key).unwrap();
        let (existing, keyless) = survey_ca(&paths).unwrap();
        assert!(existing.is_none());
        assert_eq!(keyless.as_ref().unwrap().sha1, ca.sha1);
        let old = old_cas(&paths, &FakeKeychain::holding(vec![ca.clone()]), None, true);
        assert_eq!(sha1s(&old.cas), vec![ca.sha1.clone()]);
        assert_eq!(
            old.cas[0].path,
            ca.retired_path(&paths),
            "planned once retired"
        );

        // setup's CA step: retire it, then create the new CA.
        keyless.unwrap().retire(&paths).unwrap();
        assert_eq!(sha1s(&CaCert::load_retired(&paths)), vec![ca.sha1.clone()]);
        let new = CaCert::create(&paths, "u", "h").unwrap();
        // The CA setup keeps is never untrusted; the retired one still is.
        let kc = FakeKeychain::holding(vec![ca.clone(), new.clone()]);
        let old = old_cas(&paths, &kc, Some(&new), true);
        assert_eq!(sha1s(&old.cas), [ca.sha1]);
    }

    /// Audit lead 1: a CA that an older lokal trusted for every use gets
    /// trusted again, which narrows it to TLS; one trusted for TLS only is
    /// left alone, so setup stays password-free (live test L01).
    #[test]
    fn setup_narrows_trust_that_covers_every_use() {
        let (_dir, _paths, ca) = installed();
        let need = |kc: FakeKeychain, ca: Option<&CaCert>| trust_need(&kc, ca).unwrap();
        assert_eq!(
            need(FakeKeychain::holding(vec![]), Some(&ca)),
            TrustNeed::Trust
        );
        assert_eq!(
            need(FakeKeychain::trusting(true), Some(&ca)),
            TrustNeed::Narrow
        );
        assert_eq!(
            need(FakeKeychain::trusting(false), Some(&ca)),
            TrustNeed::Done
        );
        // No usable CA yet: the new one needs trusting.
        assert_eq!(need(FakeKeychain::trusting(false), None), TrustNeed::Trust);
        assert!(TrustNeed::Trust.needs_privilege());
        assert!(TrustNeed::Narrow.needs_privilege());
        assert!(!TrustNeed::Done.needs_privilege());
    }
}
