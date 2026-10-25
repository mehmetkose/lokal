//! A repo's `lokal.toml` (SPEC §4.11): read it, check every rule, and say
//! where in the file each problem is.
//!
//! The document is parsed into a `toml::de::DeTable` and walked by hand
//! rather than through serde: every key and value keeps its span, so each
//! error and warning names its line and column, the short form (a string)
//! and the bare-number error are handled directly, and unknown keys are
//! found. Errors are usage errors (exit 2), `<path>:<line>:<col>: <message>`;
//! warnings are `<path>:<line>:<col>: unknown key `x` (ignored)`, for the
//! caller to print.

use std::collections::BTreeMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use toml::Spanned;
use toml::de::{DeString, DeTable, DeValue};

use crate::error::UserError;
use crate::paths::{PROJECT_FILE, Paths};
use crate::state::{HostHeader, validate};
use crate::util;

/// A checked `lokal.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFile {
    /// Where it was found.
    pub path: PathBuf,
    /// The real file: `path` with every symlink and `..` resolved. A
    /// person's approval is keyed by it (SPEC §16.11).
    pub canonical: PathBuf,
    /// The SHA-256 (hex) of the bytes that were parsed, so the bytes a
    /// person approves are the bytes that run (SPEC §16.11).
    pub digest: String,
    /// Its directory: the default `cwd`, and the source of a derived name.
    pub dir: PathBuf,
    /// A valid project name (a §4.3 label).
    pub project: String,
    /// `project` came from the directory name, not from the file.
    pub project_derived: bool,
    /// The validated apex, only when the file sets `domain`.
    pub domain: Option<String>,
    /// `domain`, or `<project>.lokal`. Service keys are normalised against
    /// it.
    pub apex: String,
    /// Normalised subdomain (`@` for the apex) → service.
    pub services: BTreeMap<String, FileService>,
    /// `<path>:<line>:<col>: unknown key `x` (ignored)`, in file order.
    pub warnings: Vec<String>,
}

/// One entry of `[services]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileService {
    /// The shell command, as written (`{port}` not yet replaced).
    pub run: String,
    /// A fixed port, or `None` for lokal to choose one.
    pub port: Option<u16>,
    /// Absolute: the file's directory joined with `cwd`. An existing
    /// directory when the file was checked.
    pub cwd: PathBuf,
    pub host_header: HostHeader,
    /// Extra environment variables, in file order.
    pub env: Vec<(String, String)>,
}

/// `note: project shop (from the directory name; …)`, printed when the
/// project name was derived.
pub fn derived_note(project: &str) -> String {
    format!(
        "project {project} (from the directory name; set project = \"…\" in {PROJECT_FILE} to choose another)"
    )
}

/// Reads and checks the `lokal.toml` at `path`. The file is read once, and
/// its digest is taken from those bytes.
pub fn load(path: &Path) -> Result<ProjectFile> {
    let text = read(path)?;
    // The approval is keyed by the real file, so a path that can't be
    // resolved is an error, never a pass.
    let canonical = std::fs::canonicalize(path)
        .with_context(|| format!("cannot resolve the real path of {}", path.display()))?;
    Ok(parse(path, &canonical, &text)?)
}

/// Only the project name of the `lokal.toml` at `path`, and whether it was
/// derived from the directory name: what `lokal down` needs. Errors in the
/// rest of the file don't matter, so a broken file still names its
/// project; a TOML syntax error or a bad `project` does.
pub fn load_project_name(path: &Path) -> Result<(String, bool)> {
    let text = read(path)?;
    let src = Source::new(path, &text);
    let doc = DeTable::parse(&text).map_err(|e| src.syntax_error(&e))?;
    Ok(project_name(&src, doc.get_ref())?)
}

fn read(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    match String::from_utf8(bytes) {
        Ok(t) => Ok(t),
        Err(e) => {
            let at = e.utf8_error().valid_up_to();
            let lossy = String::from_utf8_lossy(e.as_bytes()).into_owned();
            let src = Source::new(path, &lossy);
            Err(src
                .error(at..at, "not valid UTF-8 (TOML files are UTF-8)")
                .into())
        }
    }
}

/// Parses and checks `text`, the contents of the `lokal.toml` at `path`
/// (whose real path is `canonical`). Only `cwd` looks at the filesystem (it
/// must be a directory).
pub fn parse(path: &Path, canonical: &Path, text: &str) -> Result<ProjectFile, UserError> {
    let src = Source::new(path, text);
    let doc = DeTable::parse(text).map_err(|e| src.syntax_error(&e))?;
    let root = doc.get_ref();
    let dir = src.dir();
    let mut warnings: Vec<(usize, String)> = vec![];
    let mut services = None;
    for (k, v) in in_file_order(root) {
        match k.get_ref().as_ref() {
            "project" | "domain" => {}
            "services" => services = Some((k, v)),
            _ => warnings.push(src.unknown_key(k)),
        }
    }
    let (project, project_derived) = project_name(&src, root)?;
    let domain = match root.get_key_value("domain") {
        Some((_, v)) => {
            let d = src.string(v, "domain", "domain = \"shop.lokal\"")?;
            Some(validate::apex(&project, Some(d)).map_err(|e| src.wrap(v.span(), e))?)
        }
        None => None,
    };
    let apex = match &domain {
        Some(d) => d.clone(),
        None => validate::apex(&project, None)?,
    };
    let services = parse_services(&src, &dir, &apex, services, &mut warnings)?;
    warnings.sort_by_key(|(at, _)| *at);
    Ok(ProjectFile {
        path: path.to_path_buf(),
        canonical: canonical.to_path_buf(),
        // `read` only checks that the bytes are UTF-8: `text` is them, unchanged.
        digest: util::sha256_hex(text.as_bytes()),
        dir,
        project,
        project_derived,
        domain,
        apex,
        services,
        warnings: warnings.into_iter().map(|(_, w)| w).collect(),
    })
}

type Entry<'a, 'i> = (&'a Spanned<DeString<'i>>, &'a Spanned<DeValue<'i>>);

/// A table's entries in the order they appear in the file (the map itself
/// may be sorted by key), so the first problem reported is the first one
/// in the file.
fn in_file_order<'a, 'i>(t: &'a DeTable<'i>) -> Vec<Entry<'a, 'i>> {
    let mut v: Vec<Entry<'a, 'i>> = t.iter().collect();
    v.sort_by_key(|(k, _)| k.span().start);
    v
}

/// `project`, or the name derived from the directory.
fn project_name(src: &Source<'_>, root: &DeTable<'_>) -> Result<(String, bool), UserError> {
    if let Some((_, v)) = root.get_key_value("project") {
        let raw = src.string(v, "project", "project = \"shop\"")?;
        let name = validate::project_name(raw).map_err(|e| {
            let e = src.wrap(v.span(), e);
            if raw.contains('.') {
                // validate's hint talks about --domain; the file has its own key.
                e.hint(format!("set domain = \"{}\" instead", domain_for(raw)))
            } else {
                e
            }
        })?;
        return Ok((name, false));
    }
    let dir = src.dir();
    let dir_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let name = derive_project_name(&dir_name);
    if !validate::is_label(&name) {
        return Err(src
            .error(
                0..0,
                format!("the directory name `{dir_name}` doesn't make a project name"),
            )
            .hint("set project = \"…\" in lokal.toml (letters, digits and '-')"));
    }
    Ok((name, true))
}

/// The directory name as a project name: lowercased, each run of
/// characters outside `[a-z0-9-]` turned into one `-`, and `-` trimmed from
/// both ends. The caller checks that the result is a label.
pub fn derive_project_name(dir_name: &str) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for c in dir_name.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// The `domain` to suggest for a dotted `project`.
fn domain_for(raw: &str) -> String {
    let d = validate::normalize(raw);
    let tld = d.rsplit('.').next().unwrap_or_default();
    if validate::SUPPORTED_TLDS.contains(&tld) {
        d
    } else {
        format!("{d}.{}", validate::DEFAULT_TLD)
    }
}

fn parse_services(
    src: &Source<'_>,
    dir: &Path,
    apex: &str,
    services: Option<Entry<'_, '_>>,
    warnings: &mut Vec<(usize, String)>,
) -> Result<BTreeMap<String, FileService>, UserError> {
    let no_services = |at: Range<usize>| {
        src.error(at, "lokal.toml has no services")
            .hint("add one entry per dev server: [services] then web = { run = \"npm run dev\" }")
    };
    let Some((key, value)) = services else {
        return Err(no_services(0..0));
    };
    let DeValue::Table(table) = value.get_ref() else {
        return Err(src
            .error(
                value.span(),
                format!(
                    "`services` must be a table, not {}",
                    a_type(value.get_ref())
                ),
            )
            .hint("[services] then web = { run = \"npm run dev\" }"));
    };
    if table.is_empty() {
        return Err(no_services(key.span()));
    }
    let mut out = BTreeMap::new();
    // Normalised subdomain → the key as written and where.
    let mut seen: BTreeMap<String, (String, Range<usize>)> = BTreeMap::new();
    // Fixed port → the service that has it and where.
    let mut ports: BTreeMap<u16, (String, Range<usize>)> = BTreeMap::new();
    for (k, v) in in_file_order(table) {
        let raw_key = src.raw(k.span());
        let sub = validate::subdomain(k.get_ref(), apex).map_err(|e| src.wrap(k.span(), e))?;
        if let Some((first, at)) = seen.get(&sub) {
            return Err(src
                .error(
                    k.span(),
                    format!(
                        "`{raw_key}` and `{first}` (line {}) are the same service, `{sub}`",
                        src.line_col(at.start).0
                    ),
                )
                .hint("keep one of them"));
        }
        seen.insert(sub.clone(), (raw_key.to_string(), k.span()));
        let svc = match v.get_ref() {
            DeValue::String(run) => FileService {
                run: src.run_command(run, v.span())?,
                port: None,
                cwd: dir.to_path_buf(),
                host_header: HostHeader::default(),
                env: vec![],
            },
            DeValue::Integer(_) => {
                let n = src.raw(v.span());
                return Err(src.error(
                    v.span(),
                    format!(
                        "`{raw_key} = {n}` has no command: every service needs `run`, \
e.g. {raw_key} = {{ run = \"npm start\", port = {n} }}"
                    ),
                ));
            }
            DeValue::Table(t) => {
                parse_service(src, dir, &sub, raw_key, k.span(), t, &mut ports, warnings)?
            }
            other => {
                return Err(src.error(
                    v.span(),
                    format!(
                        "service `{sub}` must be a command or a table like {{ run = \"…\" }}, not {}",
                        a_type(other)
                    ),
                ));
            }
        };
        out.insert(sub, svc);
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn parse_service(
    src: &Source<'_>,
    dir: &Path,
    sub: &str,
    raw_key: &str,
    key_span: Range<usize>,
    t: &DeTable<'_>,
    ports: &mut BTreeMap<u16, (String, Range<usize>)>,
    warnings: &mut Vec<(usize, String)>,
) -> Result<FileService, UserError> {
    let mut svc = FileService {
        run: String::new(),
        port: None,
        cwd: dir.to_path_buf(),
        host_header: HostHeader::default(),
        env: vec![],
    };
    let mut has_run = false;
    for (k, v) in in_file_order(t) {
        match k.get_ref().as_ref() {
            "run" => {
                let run = src.string(v, "run", "run = \"npm start\"")?;
                svc.run = src.run_command(run, v.span())?;
                has_run = true;
            }
            "port" => {
                let port = src.port(v)?;
                if let Some((other, at)) = ports.get(&port) {
                    return Err(src
                        .error(
                            v.span(),
                            format!(
                                "port {port} is already the port of `{other}` (line {})",
                                src.line_col(at.start).0
                            ),
                        )
                        .hint(
                            "give each service its own port, or leave port out and lokal picks a free one",
                        ));
                }
                ports.insert(port, (sub.to_string(), v.span()));
                svc.port = Some(port);
            }
            "cwd" => {
                let cwd = src.string(v, "cwd", "cwd = \"services/api\"")?;
                let abs = Paths::project_cwd(dir, cwd);
                if !abs.is_dir() {
                    return Err(src
                        .error(
                            v.span(),
                            format!("cwd `{cwd}` is not a directory: {}", abs.display()),
                        )
                        .hint("cwd is relative to the directory of lokal.toml"));
                }
                svc.cwd = abs;
            }
            "host_header" => {
                svc.host_header =
                    match src.string(v, "host_header", "host_header = \"upstream\"")? {
                        "preserve" => HostHeader::Preserve,
                        "upstream" => HostHeader::Upstream,
                        _ => {
                            return Err(src.error(
                                v.span(),
                                "`host_header` must be \"preserve\" or \"upstream\"",
                            ));
                        }
                    };
            }
            "env" => svc.env = src.env(v)?,
            _ => warnings.push(src.unknown_key(k)),
        }
    }
    if !has_run {
        let e = src.error(
            key_span,
            format!(
                "service `{sub}` has no `run`: every service needs a command, \
e.g. {raw_key} = {{ run = \"npm start\" }}"
            ),
        );
        // `v1.api = …` unquoted is a table `v1` holding `api`.
        let nested = t.values().any(|v| v.get_ref().is_table());
        return Err(if nested {
            e.hint("a subdomain with a dot needs quotes: \"v1.api\" = { run = \"…\" }")
        } else {
            e
        });
    }
    Ok(svc)
}

/// The variables lokal sets for every command it starts (SPEC §4.10).
fn reserved_env(name: &str) -> bool {
    matches!(name, "PORT" | "HOST" | "LOKAL_URL") || name.starts_with("LOKAL_URL_")
}

/// `a string`, `an integer`, … for messages.
fn a_type(v: &DeValue<'_>) -> &'static str {
    match v {
        DeValue::String(_) => "a string",
        DeValue::Integer(_) => "an integer",
        DeValue::Float(_) => "a float",
        DeValue::Boolean(_) => "a boolean",
        DeValue::Datetime(_) => "a date",
        DeValue::Array(_) => "an array",
        DeValue::Table(_) => "a table",
    }
}

/// The file being checked: turns byte offsets into `<path>:<line>:<col>`.
struct Source<'a> {
    path: &'a Path,
    text: &'a str,
}

impl<'a> Source<'a> {
    fn new(path: &'a Path, text: &'a str) -> Self {
        Self { path, text }
    }

    fn dir(&self) -> PathBuf {
        self.path.parent().unwrap_or(Path::new("/")).to_path_buf()
    }

    /// 1-based line (counting `\n`) and column (counting characters).
    fn line_col(&self, offset: usize) -> (usize, usize) {
        let mut at = offset.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        let before = &self.text[..at];
        let line = before.matches('\n').count() + 1;
        let col = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
        (line, col)
    }

    /// `<path>:<line>:<col>`
    fn at(&self, offset: usize) -> String {
        let (line, col) = self.line_col(offset);
        format!("{}:{line}:{col}", self.path.display())
    }

    /// The text of `span` as written in the file (`"@"` keeps its quotes).
    fn raw(&self, span: Range<usize>) -> &'a str {
        self.text.get(span).unwrap_or_default().trim()
    }

    fn error(&self, span: Range<usize>, msg: impl AsRef<str>) -> UserError {
        UserError::usage(format!("{}: {}", self.at(span.start), msg.as_ref()))
    }

    /// A validation error from §4.3, placed at `span`. Keeps its hint.
    fn wrap(&self, span: Range<usize>, e: UserError) -> UserError {
        UserError {
            message: format!("{}: {}", self.at(span.start), e.message),
            ..e
        }
    }

    fn syntax_error(&self, e: &toml::de::Error) -> UserError {
        let msg = e.message().trim_end();
        match e.span() {
            Some(span) => self.error(span, msg),
            None => UserError::usage(format!("{}: {msg}", self.path.display())),
        }
    }

    fn unknown_key(&self, k: &Spanned<DeString<'_>>) -> (usize, String) {
        let at = k.span().start;
        (
            at,
            format!("{}: unknown key `{}` (ignored)", self.at(at), k.get_ref()),
        )
    }

    fn string<'v>(
        &self,
        v: &'v Spanned<DeValue<'_>>,
        key: &str,
        example: &str,
    ) -> Result<&'v str, UserError> {
        v.get_ref().as_str().ok_or_else(|| {
            self.error(
                v.span(),
                format!("`{key}` must be a string, not {}", a_type(v.get_ref())),
            )
            .hint(format!("for example: {example}"))
        })
    }

    /// `run`: not empty, and nothing a command line can't hold.
    fn run_command(&self, run: &str, span: Range<usize>) -> Result<String, UserError> {
        if run.trim().is_empty() {
            return Err(self
                .error(span, "`run` is empty: every service needs a command")
                .hint("for example: run = \"npm start\""));
        }
        if run.contains('\0') {
            return Err(self.error(span, "`run` can't contain a NUL character"));
        }
        Ok(run.to_string())
    }

    /// `port`: an integer by the §4.3 rules for a loopback host. The range
    /// is checked before converting from TOML's 64-bit integer.
    fn port(&self, v: &Spanned<DeValue<'_>>) -> Result<u16, UserError> {
        let DeValue::Integer(i) = v.get_ref() else {
            return Err(self
                .error(
                    v.span(),
                    format!("`port` must be a number, not {}", a_type(v.get_ref())),
                )
                .hint("for example: port = 3000, or leave it out and lokal picks a free one"));
        };
        match i64::from_str_radix(i.as_str(), i.radix()) {
            Ok(n) if (1..=65535).contains(&n) => {
                validate::port(n as u32, "localhost").map_err(|e| self.wrap(v.span(), e))
            }
            _ => Err(self.error(
                v.span(),
                format!("port {} is out of range (1–65535)", self.raw(v.span())),
            )),
        }
    }

    /// `env`: a table of strings, without the variables lokal sets.
    fn env(&self, v: &Spanned<DeValue<'_>>) -> Result<Vec<(String, String)>, UserError> {
        let example = "env = { NODE_ENV = \"development\" }";
        let DeValue::Table(t) = v.get_ref() else {
            return Err(self
                .error(
                    v.span(),
                    format!(
                        "`env` must be a table of strings, not {}",
                        a_type(v.get_ref())
                    ),
                )
                .hint(format!("for example: {example}")));
        };
        let mut env = vec![];
        for (k, val) in in_file_order(t) {
            let name: &str = k.get_ref();
            if reserved_env(name) {
                return Err(self
                    .error(
                        k.span(),
                        format!(
                            "env `{name}` is set by lokal (it sets PORT, HOST, LOKAL_URL and LOKAL_URL_<SUB>); use `port` instead"
                        ),
                    )
                    .hint("port = 3000 fixes the port; without it lokal picks one and sets PORT"));
            }
            if name.is_empty() || name.contains(['=', '\0']) {
                return Err(self.error(
                    k.span(),
                    format!("`{name}` is not a valid environment variable name"),
                ));
            }
            let Some(value) = val.get_ref().as_str() else {
                return Err(self
                    .error(
                        val.span(),
                        format!(
                            "env `{name}` must be a string, not {}",
                            a_type(val.get_ref())
                        ),
                    )
                    .hint(format!(
                        "quote it: {name} = \"{}\"",
                        self.raw(val.span()).replace('"', "\\\"")
                    )));
            };
            if value.contains('\0') {
                return Err(self.error(
                    val.span(),
                    format!("env `{name}` can't contain a NUL character"),
                ));
            }
            env.push((name.to_string(), value.to_string()));
        }
        Ok(env)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repo directory with a `lokal.toml` path in it (the file itself
    /// isn't needed: `parse` takes the text).
    struct Repo {
        _root: tempfile::TempDir,
        dir: PathBuf,
    }

    impl Repo {
        fn new(name: &str) -> Self {
            let root = tempfile::tempdir().unwrap();
            let dir = root.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            Self { _root: root, dir }
        }

        fn file(&self) -> PathBuf {
            self.dir.join(PROJECT_FILE)
        }

        fn parse(&self, text: &str) -> Result<ProjectFile, UserError> {
            parse(&self.file(), &self.file(), text)
        }

        fn ok(&self, text: &str) -> ProjectFile {
            self.parse(text)
                .unwrap_or_else(|e| panic!("{}\nhint: {:?}", e.message, e.hint))
        }

        /// The error, which must be a usage error at `line:col`.
        fn err(&self, text: &str, line: usize, col: usize) -> UserError {
            let e = self.parse(text).expect_err("should fail");
            assert_eq!(e.code, crate::error::EXIT_USAGE, "{}", e.message);
            let at = format!("{}:{line}:{col}: ", self.file().display());
            assert!(e.message.starts_with(&at), "want {at}, got {}", e.message);
            e
        }
    }

    const SPEC_EXAMPLE: &str = r#"# lokal.toml at the repo root
project = "shop"                 # optional; default: the directory name
# domain = "shop.lokal"          # optional; default: <project>.lokal

[services]
"@"  = { run = "npm start", port = 3000 }                    # https://shop.lokal, fixed port
api  = { run = "cargo run -p api", cwd = "services/api" }    # lokal picks a free port
web  = { run = "vite --port {port}", host_header = "upstream" }
docs = "mkdocs serve -a 127.0.0.1:{port}"                    # short form: the string is the command
"#;

    #[test]
    fn the_spec_example_parses() {
        let r = Repo::new("whatever");
        std::fs::create_dir_all(r.dir.join("services/api")).unwrap();
        let f = r.ok(SPEC_EXAMPLE);
        assert_eq!(f.path, r.file());
        assert_eq!(f.dir, r.dir);
        assert_eq!(f.project, "shop");
        assert!(!f.project_derived);
        assert_eq!(f.domain, None);
        assert_eq!(f.apex, "shop.lokal");
        assert!(f.warnings.is_empty(), "{:?}", f.warnings);
        assert_eq!(
            f.services.keys().collect::<Vec<_>>(),
            ["@", "api", "docs", "web"]
        );
        let at = &f.services["@"];
        assert_eq!(at.run, "npm start");
        assert_eq!(at.port, Some(3000));
        assert_eq!(at.cwd, r.dir);
        assert_eq!(at.host_header, HostHeader::Preserve);
        let api = &f.services["api"];
        assert_eq!(api.port, None);
        assert_eq!(api.cwd, r.dir.join("services/api"));
        assert_eq!(f.services["web"].host_header, HostHeader::Upstream);
        assert_eq!(f.services["web"].run, "vite --port {port}");
        // The short form: the string is `run`.
        assert_eq!(
            f.services["docs"],
            FileService {
                run: "mkdocs serve -a 127.0.0.1:{port}".into(),
                port: None,
                cwd: r.dir.clone(),
                host_header: HostHeader::Preserve,
                env: vec![],
            }
        );
    }

    #[test]
    fn sub_tables_work_like_inline_tables() {
        let r = Repo::new("x");
        let f = r.ok(
            "project = \"p\"\n[services.api]\nrun = \"cargo run\"\nport = 4000\n[services.api.env]\nRUST_LOG = \"debug\"\n",
        );
        assert_eq!(f.services["api"].port, Some(4000));
        assert_eq!(
            f.services["api"].env,
            [("RUST_LOG".to_string(), "debug".to_string())]
        );
    }

    #[test]
    fn project_name_from_the_directory() {
        for (dir, want) in [
            ("shop", "shop"),
            ("My Shop", "my-shop"),
            ("my_shop.v2", "my-shop-v2"),
            ("__Shop__", "shop"),
            ("a--b", "a--b"),
            ("Café Ünïcode", "caf-n-code"),
            ("123", "123"),
        ] {
            assert_eq!(derive_project_name(dir), want, "{dir}");
            let r = Repo::new(dir);
            let f = r.ok("[services]\nweb = \"npm run dev\"\n");
            assert_eq!(f.project, want, "{dir}");
            assert!(f.project_derived);
            assert_eq!(f.apex, format!("{want}.lokal"));
        }
        // A name that can't become a label asks for `project`.
        for dir in ["___", "ŞÇ", &"a".repeat(64)] {
            let r = Repo::new(dir);
            let e = r.err("[services]\nweb = \"x\"\n", 1, 1);
            assert!(e.message.contains("doesn't make a project name"), "{e}");
            assert!(e.hint.unwrap().contains("project = \"…\""));
        }
        assert!(derived_note("shop").starts_with("project shop (from the directory name;"));
    }

    #[test]
    fn project_key_rules() {
        let r = Repo::new("dir");
        let f = r.ok("project = \"  Shop \"\n[services]\nweb = \"x\"\n");
        assert_eq!(f.project, "shop");
        assert!(!f.project_derived);
        // A dotted name: the file's own hint, not --domain.
        let e = r.err("project = \"shop.lokal\"\n[services]\nweb = \"x\"\n", 1, 11);
        assert!(e.message.contains("can't contain dots"), "{e}");
        assert_eq!(
            e.hint.as_deref(),
            Some("set domain = \"shop.lokal\" instead")
        );
        let e = r.err("project = \"shop.acme\"\n[services]\nweb = \"x\"\n", 1, 11);
        assert_eq!(
            e.hint.as_deref(),
            Some("set domain = \"shop.acme.lokal\" instead")
        );
        let e = r.err("project = \"-x\"\n[services]\nweb = \"x\"\n", 1, 11);
        assert!(e.message.contains("invalid project name `-x`"), "{e}");
        let e = r.err("project = 5\n[services]\nweb = \"x\"\n", 1, 11);
        assert!(
            e.message
                .contains("`project` must be a string, not an integer"),
            "{e}"
        );
    }

    #[test]
    fn domain_rules() {
        let r = Repo::new("dir");
        let f = r.ok("project = \"shop\"\ndomain = \"Shop.Acme.LOCAL\"\n[services]\nweb = \"x\"\n");
        assert_eq!(f.domain.as_deref(), Some("shop.acme.local"));
        assert_eq!(f.apex, "shop.acme.local");
        let e = r.err(
            "project = \"shop\"\ndomain = \"shop.test\"\n[services]\nweb = \"x\"\n",
            2,
            10,
        );
        assert!(e.message.contains("only .lokal and .local"), "{e}");
        r.err("domain = \"lokal\"\n[services]\nweb = \"x\"\n", 1, 10);
        r.err("domain = true\n[services]\nweb = \"x\"\n", 1, 10);
        // Service keys are normalised against the file's apex.
        let f = r
            .ok("domain = \"a.local\"\n[services]\n\"api.a.local\" = \"x\"\n\"a.local\" = \"y\"\n");
        assert_eq!(f.services.keys().collect::<Vec<_>>(), ["@", "api"]);
    }

    #[test]
    fn no_services_is_an_error() {
        let r = Repo::new("dir");
        let e = r.err("project = \"p\"\n", 1, 1);
        assert!(e.message.ends_with(": lokal.toml has no services"), "{e}");
        let e = r.err("project = \"p\"\n[services]\n", 2, 2);
        assert!(e.message.ends_with(": lokal.toml has no services"), "{e}");
        r.err("project = \"p\"\nservices = {}\n", 2, 1);
        let e = r.err("services = \"npm start\"\n", 1, 12);
        assert!(
            e.message
                .contains("`services` must be a table, not a string"),
            "{e}"
        );
    }

    #[test]
    fn service_keys_are_subdomains() {
        let r = Repo::new("dir");
        let f = r.ok("project = \"p\"\n[services]\nAPI = \"a\"\n\"v1.api\" = \"b\"\n\"web.p.lokal\" = \"c\"\n\"@\" = \"d\"\n");
        assert_eq!(
            f.services.keys().collect::<Vec<_>>(),
            ["@", "api", "v1.api", "web"]
        );
        let e = r.err(
            "project = \"p\"\n[services]\nok = \"a\"\n\"bad_name\" = \"b\"\n",
            4,
            1,
        );
        assert!(
            e.message.contains("invalid subdomain label `bad_name`"),
            "{e}"
        );
        r.err("project = \"p\"\n[services]\n\"-x\" = \"b\"\n", 3, 1);
    }

    #[test]
    fn two_keys_for_one_subdomain_name_both_lines() {
        let r = Repo::new("dir");
        let e = r.err(
            "project = \"shop\"\n[services]\napi = \"a\"\n\nAPI = \"b\"\n",
            5,
            1,
        );
        assert!(
            e.message
                .contains("`API` and `api` (line 3) are the same service, `api`"),
            "{e}"
        );
        let e = r.err(
            "project = \"shop\"\n[services]\napi = \"a\"\n\"api.shop.lokal\" = \"b\"\n",
            4,
            1,
        );
        assert!(e.message.contains("(line 3)"), "{e}");
        let e = r.err(
            "project = \"shop\"\n[services]\n\"@\" = \"a\"\n\"shop.lokal\" = \"b\"\n",
            4,
            1,
        );
        assert!(e.message.contains("the same service, `@`"), "{e}");
    }

    #[test]
    fn service_values() {
        let r = Repo::new("dir");
        let e = r.err("project = \"p\"\n[services]\napi = 4000\n", 3, 7);
        assert!(
            e.message.ends_with(
                ": `api = 4000` has no command: every service needs `run`, e.g. api = { run = \"npm start\", port = 4000 }"
            ),
            "{e}"
        );
        let e = r.err("project = \"p\"\n[services]\n\"@\" = 3000\n", 3, 7);
        assert!(e.message.contains("`\"@\" = 3000` has no command"), "{e}");
        for (v, ty) in [
            ("true", "a boolean"),
            ("[\"a\"]", "an array"),
            ("1.5", "a float"),
            ("2026-10-25", "a date"),
        ] {
            let e = r.err(&format!("project = \"p\"\n[services]\napi = {v}\n"), 3, 7);
            assert!(
                e.message.contains(&format!(
                    "service `api` must be a command or a table like {{ run = \"…\" }}, not {ty}"
                )),
                "{e}"
            );
        }
    }

    #[test]
    fn run_rules() {
        let r = Repo::new("dir");
        let e = r.err("project = \"p\"\n[services]\napi = { port = 4000 }\n", 3, 1);
        assert!(e.message.contains("service `api` has no `run`"), "{e}");
        assert!(
            e.message.contains("e.g. api = { run = \"npm start\" }"),
            "{e}"
        );
        // An unquoted dotted key is a nested table: say so.
        let e = r.err(
            "project = \"p\"\n[services]\nv1.api = { run = \"x\" }\n",
            3,
            1,
        );
        assert!(e.message.contains("service `v1` has no `run`"), "{e}");
        assert!(e.hint.unwrap().contains("\"v1.api\""));
        for empty in ["\"\"", "\"   \""] {
            let e = r.err(
                &format!("project = \"p\"\n[services]\napi = {{ run = {empty} }}\n"),
                3,
                15,
            );
            assert!(e.message.contains("`run` is empty"), "{e}");
            let e = r.err(
                &format!("project = \"p\"\n[services]\napi = {empty}\n"),
                3,
                7,
            );
            assert!(e.message.contains("`run` is empty"), "{e}");
        }
        let e = r.err(
            "project = \"p\"\n[services]\napi = { run = [\"a\"] }\n",
            3,
            15,
        );
        assert!(
            e.message.contains("`run` must be a string, not an array"),
            "{e}"
        );
        r.err("project = \"p\"\n[services]\napi = \"a\\u0000b\"\n", 3, 7);
        let f =
            r.ok("project = \"p\"\n[services]\napi = { run = \"  npm start | tee x && y \" }\n");
        assert_eq!(f.services["api"].run, "  npm start | tee x && y ");
    }

    #[test]
    fn port_rules() {
        let r = Repo::new("dir");
        let svc = |port: &str| {
            format!("project = \"p\"\n[services]\napi = {{ run = \"x\", port = {port} }}\n")
        };
        assert_eq!(r.ok(&svc("4000")).services["api"].port, Some(4000));
        assert_eq!(r.ok(&svc("0x1f40")).services["api"].port, Some(8000));
        assert_eq!(r.ok(&svc("65_535")).services["api"].port, Some(65535));
        for bad in [
            "0",
            "-1",
            "65536",
            "99999999999999999999",
            "-9223372036854775808",
        ] {
            let e = r.err(&svc(bad), 3, 27);
            assert!(
                e.message
                    .contains(&format!("port {bad} is out of range (1–65535)")),
                "{e}"
            );
        }
        for own in ["80", "443"] {
            let e = r.err(&svc(own), 3, 27);
            assert!(e.message.contains("is lokal itself"), "{e}");
        }
        let e = r.err(&svc("\"4000\""), 3, 27);
        assert!(
            e.message.contains("`port` must be a number, not a string"),
            "{e}"
        );
        r.err(&svc("40.5"), 3, 27);
        // Two services with one port.
        let e = r.err(
            "project = \"p\"\n[services]\n\"@\" = { run = \"a\", port = 3000 }\nweb = { run = \"b\", port = 3000 }\n",
            4,
            27,
        );
        assert!(
            e.message
                .contains("port 3000 is already the port of `@` (line 3)"),
            "{e}"
        );
    }

    #[test]
    fn cwd_rules() {
        let r = Repo::new("dir");
        std::fs::create_dir_all(r.dir.join("services/api")).unwrap();
        std::fs::write(r.dir.join("file"), "").unwrap();
        let svc = |cwd: &str| {
            format!("project = \"p\"\n[services]\napi = {{ run = \"x\", cwd = {cwd} }}\n")
        };
        assert_eq!(
            r.ok(&svc("\"services/api\"")).services["api"].cwd,
            r.dir.join("services/api")
        );
        assert_eq!(r.ok(&svc("\".\"")).services["api"].cwd, r.dir);
        assert_eq!(
            r.ok(&svc("\"./services/../services/api\"")).services["api"].cwd,
            r.dir.join("services/../services/api")
        );
        let e = r.err(&svc("\"services/web\""), 3, 26);
        assert!(
            e.message
                .contains("cwd `services/web` is not a directory: "),
            "{e}"
        );
        assert!(
            e.message
                .ends_with(&r.dir.join("services/web").display().to_string())
        );
        r.err(&svc("\"file\""), 3, 26);
        r.err(&svc("5"), 3, 26);
    }

    #[test]
    fn host_header_rules() {
        let r = Repo::new("dir");
        let svc = |hh: &str| {
            format!("project = \"p\"\n[services]\nweb = {{ run = \"x\", host_header = {hh} }}\n")
        };
        assert_eq!(
            r.ok(&svc("\"preserve\"")).services["web"].host_header,
            HostHeader::Preserve
        );
        assert_eq!(
            r.ok(&svc("\"upstream\"")).services["web"].host_header,
            HostHeader::Upstream
        );
        let e = r.err(&svc("\"rewrite\""), 3, 34);
        assert!(e.message.contains("\"preserve\" or \"upstream\""), "{e}");
        r.err(&svc("true"), 3, 34);
    }

    #[test]
    fn env_rules() {
        let r = Repo::new("dir");
        let svc = |env: &str| {
            format!("project = \"p\"\n[services]\nweb = {{ run = \"x\", env = {env} }}\n")
        };
        let f = r.ok(&svc(
            "{ NODE_ENV = \"development\", A = \"1\", port = \"lower case is not lokal's\" }",
        ));
        assert_eq!(
            f.services["web"].env,
            [
                ("NODE_ENV".to_string(), "development".to_string()),
                ("A".to_string(), "1".to_string()),
                ("port".to_string(), "lower case is not lokal's".to_string()),
            ]
        );
        for name in ["PORT", "HOST", "LOKAL_URL", "LOKAL_URL_API", "LOKAL_URL_"] {
            let e = r.err(&svc(&format!("{{ {name} = \"1\" }}")), 3, 28);
            assert!(
                e.message.contains(&format!("env `{name}` is set by lokal")),
                "{e}"
            );
            assert!(e.message.contains("use `port` instead"), "{e}");
        }
        let e = r.err(&svc("{ DEBUG = 1 }"), 3, 36);
        assert!(
            e.message
                .contains("env `DEBUG` must be a string, not an integer"),
            "{e}"
        );
        assert_eq!(e.hint.as_deref(), Some("quote it: DEBUG = \"1\""));
        r.err(&svc("{ X = true }"), 3, 32);
        r.err(&svc("{ X = { Y = \"z\" } }"), 3, 32);
        let e = r.err(&svc("\"NODE_ENV=dev\""), 3, 26);
        assert!(
            e.message
                .contains("`env` must be a table of strings, not a string"),
            "{e}"
        );
        r.err(&svc("{ \"A=B\" = \"1\" }"), 3, 28);
        r.err(&svc("{ \"\" = \"1\" }"), 3, 28);
    }

    #[test]
    fn unknown_keys_warn_with_their_location_in_file_order() {
        let r = Repo::new("dir");
        let f = r.ok(
            "project = \"p\"\nversion = 2\n[services]\nweb = { run = \"x\", restart = true, host = \"0.0.0.0\" }\n[tool]\nx = 1\n",
        );
        let file = r.file().display().to_string();
        assert_eq!(
            f.warnings,
            [
                format!("{file}:2:1: unknown key `version` (ignored)"),
                format!("{file}:4:20: unknown key `restart` (ignored)"),
                format!("{file}:4:36: unknown key `host` (ignored)"),
                format!("{file}:5:2: unknown key `tool` (ignored)"),
            ]
        );
        assert_eq!(f.services["web"].run, "x");
    }

    #[test]
    fn a_syntax_error_has_the_parsers_location() {
        let r = Repo::new("dir");
        let e = r.err("project = \"p\"\n[services]\nweb = { run = \"x\" \n", 3, 18);
        assert!(!e.message.contains('\n'), "{e}");
        let e = r.err(
            "project = \"p\"\nproject = \"q\"\n[services]\nweb = \"x\"\n",
            2,
            1,
        );
        assert!(e.message.contains("duplicate key"), "{e}");
    }

    #[test]
    fn columns_count_characters_not_bytes() {
        let r = Repo::new("dir");
        let e = r.err(
            "# ünïcödé\nproject = \"p\"\n[services]\n\"çafé\" = \"x\"\n",
            4,
            1,
        );
        assert!(e.message.contains("`çafé`"), "{e}");
        let e = r.err(
            "project = \"p\" # ü\n[services]\nweb = { run = \"ü\", port = 0 }\n",
            3,
            27,
        );
        assert!(e.message.contains("out of range"), "{e}");
        let src = Source::new(Path::new("f"), "aé\nbü c");
        assert_eq!(src.line_col(0), (1, 1));
        assert_eq!(src.line_col(1), (1, 2));
        assert_eq!(src.line_col(3), (1, 3));
        assert_eq!(src.line_col(4), (2, 1));
        assert_eq!(src.line_col(8), (2, 4));
        // Inside a character, and past the end.
        assert_eq!(src.line_col(2), (1, 2));
        assert_eq!(src.line_col(99), (2, 5));
    }

    #[test]
    fn load_reads_the_file_and_project_name_survives_other_errors() {
        let r = Repo::new("My Repo");
        std::fs::write(r.file(), "project = \"shop\"\n[services]\napi = 4000\n").unwrap();
        let e = load(&r.file()).unwrap_err();
        let u = e.downcast_ref::<UserError>().unwrap();
        assert_eq!(u.code, crate::error::EXIT_USAGE);
        assert!(u.message.contains("has no command"), "{u}");
        assert_eq!(
            load_project_name(&r.file()).unwrap(),
            ("shop".to_string(), false)
        );
        std::fs::write(r.file(), "[services]\n").unwrap();
        assert_eq!(
            load_project_name(&r.file()).unwrap(),
            ("my-repo".to_string(), true)
        );
        std::fs::write(r.file(), "project = \"a.b\"\n").unwrap();
        assert!(load_project_name(&r.file()).is_err());
        std::fs::write(r.file(), "project = \n").unwrap();
        assert!(load_project_name(&r.file()).is_err());
        std::fs::write(r.file(), "project = \"shop\"\n[services]\nweb = \"x\"\n").unwrap();
        assert_eq!(load(&r.file()).unwrap().project, "shop");
        // Not UTF-8: a usage error at the first bad byte.
        std::fs::write(r.file(), b"project = \"p\"\n# \xff\n").unwrap();
        let e = load(&r.file()).unwrap_err();
        let u = e.downcast_ref::<UserError>().unwrap();
        assert!(
            u.message
                .starts_with(&format!("{}:2:3: not valid UTF-8", r.file().display())),
            "{u}"
        );
    }

    /// SPEC §16.11: the approval is of the bytes that were parsed, and is
    /// keyed by the real file, which a symlink or `..` leads to.
    #[test]
    fn load_hashes_the_parsed_bytes_and_resolves_the_real_path() {
        let r = Repo::new("shop");
        let text = "project = \"shop\"\n[services]\nweb = \"x\" # ü\n";
        std::fs::write(r.file(), text).unwrap();
        let f = load(&r.file()).unwrap();
        assert_eq!(f.digest, util::sha256_hex(text.as_bytes()));
        assert_eq!(f.digest.len(), 64);
        let real = r.file().canonicalize().unwrap();
        assert_eq!(f.canonical, real);
        // Any change, even a comment, changes the digest.
        std::fs::write(r.file(), format!("{text}#\n")).unwrap();
        assert_ne!(load(&r.file()).unwrap().digest, f.digest);
        std::fs::write(r.file(), text).unwrap();
        // Through a symlinked directory, a symlinked file, and `..`.
        let root = r.dir.parent().unwrap();
        std::os::unix::fs::symlink(&r.dir, root.join("link")).unwrap();
        std::fs::create_dir(root.join("other")).unwrap();
        std::os::unix::fs::symlink(r.file(), root.join("other").join(PROJECT_FILE)).unwrap();
        for via in [
            root.join("link").join(PROJECT_FILE),
            root.join("other").join(PROJECT_FILE),
            root.join("other/../shop").join(PROJECT_FILE),
        ] {
            let g = load(&via).unwrap();
            assert_eq!(g.path, via);
            assert_eq!(g.canonical, real, "{}", via.display());
            assert_eq!(g.digest, f.digest);
        }
    }
}
