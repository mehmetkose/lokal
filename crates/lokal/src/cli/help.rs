//! Root help (SPEC §4.2). The command list is generated from the clap tree
//! at runtime, so it can never drift from the real commands; a test asserts
//! every non-hidden leaf command appears.

use clap::{Command, CommandFactory};

use super::args::Cli;

const WIDTH: usize = 33;

/// Section of each top-level command. Commands not listed land in
/// "Other commands", so new commands still show up.
const SECTIONS: &[(&str, &[&str])] = &[
    ("Get started", &["setup", "doctor", "uninstall"]),
    ("Projects", &["project"]),
    ("Services", &["service", "run", "list"]),
    (
        "Background service",
        &["start", "stop", "restart", "status", "logs", "upgrade"],
    ),
    ("Utilities", &["env", "ca", "open"]),
];

/// Commands shown on one line with a shared description.
const COMBINED: (&[&str], &str) = (
    &["start", "stop", "restart"],
    "Control the background service",
);

/// One visible leaf: its path (`project create`), positional usage and about.
#[derive(Debug, Clone)]
pub struct Leaf {
    pub top: String,
    pub path: String,
    pub usage: String,
    pub about: String,
}

fn positionals(cmd: &Command) -> String {
    let mut parts = vec![];
    for a in cmd.get_positionals() {
        if a.is_hide_set() {
            continue;
        }
        let name = a
            .get_value_names()
            .and_then(|v| v.first().map(|s| s.to_string()))
            .unwrap_or_else(|| a.get_id().as_str().to_ascii_uppercase());
        // A `last` argument comes after `--` and takes the rest
        // (`run <SUBDOMAIN> -- <COMMAND>...`).
        let part = match (a.is_last_set(), a.is_required_set()) {
            (true, true) => format!("-- <{name}>..."),
            (true, false) => format!("[-- <{name}>...]"),
            (false, true) => format!("<{name}>"),
            (false, false) => format!("[{name}]"),
        };
        parts.push(part);
    }
    parts.join(" ")
}

/// Every non-hidden leaf command, in tree order.
pub fn leaves(root: &Command) -> Vec<Leaf> {
    let mut v = vec![];
    for top in root.get_subcommands() {
        if top.is_hide_set() || top.get_name() == "help" {
            continue;
        }
        let subs: Vec<&Command> = top
            .get_subcommands()
            .filter(|c| !c.is_hide_set() && c.get_name() != "help")
            .collect();
        if subs.is_empty() {
            v.push(Leaf {
                top: top.get_name().into(),
                path: top.get_name().into(),
                usage: positionals(top),
                about: top.get_about().map(|s| s.to_string()).unwrap_or_default(),
            });
        } else {
            for s in subs {
                v.push(Leaf {
                    top: top.get_name().into(),
                    path: format!("{} {}", top.get_name(), s.get_name()),
                    usage: positionals(s),
                    about: s.get_about().map(|s| s.to_string()).unwrap_or_default(),
                });
            }
        }
    }
    v
}

fn line(label: &str, about: &str) -> String {
    let w = WIDTH.max(label.chars().count() + 2);
    format!("  {label:<w$}{about}\n")
}

/// The generated "Get started / Projects / …" block.
pub fn command_sections(root: &Command) -> String {
    let all = leaves(root);
    let mut used = vec![false; all.len()];
    let mut out = String::new();
    let mut sections: Vec<(&str, Vec<&str>)> =
        SECTIONS.iter().map(|(t, c)| (*t, c.to_vec())).collect();
    sections.push(("Other commands", vec![]));
    for (title, tops) in &sections {
        let mut body = String::new();
        let mut combined_done = false;
        // Ordered by the section's list, then tree order within a top-level command.
        let order: Vec<usize> = if tops.is_empty() {
            (0..all.len()).filter(|i| !used[*i]).collect()
        } else {
            let all = &all;
            tops.iter()
                .flat_map(|t| (0..all.len()).filter(move |i| all[*i].top == *t))
                .collect()
        };
        for i in order {
            if used[i] {
                continue;
            }
            let leaf = &all[i];
            if COMBINED.0.contains(&leaf.path.as_str()) {
                let present: Vec<&str> = COMBINED
                    .0
                    .iter()
                    .copied()
                    .filter(|c| all.iter().any(|l| l.path == *c))
                    .collect();
                for (j, l) in all.iter().enumerate() {
                    if present.contains(&l.path.as_str()) {
                        used[j] = true;
                    }
                }
                if !combined_done {
                    body.push_str(&line(&present.join(" | "), COMBINED.1));
                    combined_done = true;
                }
                continue;
            }
            used[i] = true;
            let label = if leaf.usage.is_empty() {
                leaf.path.clone()
            } else {
                format!("{} {}", leaf.path, leaf.usage)
            };
            body.push_str(&line(&label, &leaf.about));
        }
        if !body.is_empty() {
            out.push_str(&format!("{title}:\n{body}\n"));
        }
    }
    out
}

const OPTIONS: &str = "Options:
  -h, --help       Print help (see more with '--help')
  -V, --version    Print version
      --no-color   Disable coloured output
  -v, --verbose    More detail on stderr
";

const FOOTER: &str = "Quick start:
  lokal setup
  lokal project create myproject
  lokal service add api 4000

Using lokal in a project (for people and coding agents):
  1. lokal status                     not set up? The user runs `lokal setup` once in their Terminal
                                      (it asks for their password; don't run it with sudo yourself)
  2. lokal project create <name>      one per repo, e.g. the repo's directory name -> <name>.lokal
  3. lokal service add <sub> <port>   one per dev server: https://<sub>.<name>.lokal -> localhost:<port>
                                      `@` is the bare https://<name>.lokal; works as soon as it returns
  4. Run the dev server on that port (127.0.0.1, ::1 or localhost all work), open the https URL.
  Or skip the port: lokal run <sub> -- <dev command> picks one, routes it and starts the server.
  Inspect: `lokal list --json`, `lokal status --json`. Problems: `lokal doctor` says what to fix.
  Node/Python/curl clients: eval \"$(lokal env)\". Vite \"host not allowed\": --host-header upstream.
  Steps 2-3 are safe to re-run. Changing a port: `lokal service update <sub> --port <port>`.

Run `lokal help <COMMAND>` for details and examples.";

/// The full root help text.
pub fn root_help_text(root: &Command) -> String {
    format!(
        "lokal {}\n{}\n\nUsage: lokal <COMMAND>\n\n{}{OPTIONS}\n{FOOTER}",
        crate::version::VERSION,
        root.get_about().map(|s| s.to_string()).unwrap_or_default(),
        command_sections(root)
    )
}

/// The clap command with the generated root help installed as its template.
pub fn command() -> Command {
    let base = Cli::command();
    let text = root_help_text(&base);
    base.help_template(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_leaf_command_is_in_root_help() {
        let root = Cli::command();
        let text = root_help_text(&root);
        let leaves = leaves(&root);
        assert!(leaves.len() >= 20, "{leaves:?}");
        for l in &leaves {
            let in_combined =
                COMBINED.0.contains(&l.path.as_str()) && text.contains(&COMBINED.0.join(" | "));
            assert!(
                in_combined
                    || text.contains(&format!("  {} ", l.path))
                    || text.contains(&format!("  {}\n", l.path)),
                "`{}` missing from root help:\n{text}",
                l.path
            );
        }
        // Spec §4.1 commands, explicitly.
        for c in [
            "setup",
            "uninstall",
            "doctor",
            "project create <NAME>",
            "project list",
            "project show [NAME]",
            "project use <NAME>",
            "project remove <NAME>",
            "service add <SUBDOMAIN> <PORT>",
            "service list",
            "service update <SUBDOMAIN>",
            "service remove <SUBDOMAIN>",
            "run <SUBDOMAIN> -- <COMMAND>",
            "list",
            "start | stop | restart",
            "status",
            "logs",
            "upgrade",
            "env",
            "ca path",
            "open <SUBDOMAIN>",
        ] {
            assert!(text.contains(c), "`{c}` missing:\n{text}");
        }
        assert!(text.contains("Run `lokal help <COMMAND>`"));
        assert!(!text.contains("daemon"), "hidden commands must not show");
        assert!(!text.contains("internal"), "hidden commands must not show");
    }

    #[test]
    fn spec_layout() {
        let text = root_help_text(&Cli::command());
        assert!(text.starts_with(&format!("lokal {}\n", crate::version::VERSION)));
        assert!(text.contains(
            "  service add <SUBDOMAIN> <PORT>   Route https://SUBDOMAIN.<project>.lokal to localhost:PORT\n"
        ));
        assert!(text.contains("  setup                            One-time onboarding"));
        // The command after `--` is part of run's usage, under "Services".
        let services = text
            .split("Services:\n")
            .nth(1)
            .and_then(|s| s.split("\n\n").next())
            .unwrap_or_default();
        assert!(
            services.contains("  run <SUBDOMAIN> -- <COMMAND>...  Pick a free port"),
            "{services}"
        );
        for s in [
            "Get started:",
            "Projects:",
            "Services:",
            "Background service:",
            "Utilities:",
            "Options:",
            "Quick start:",
            "Using lokal in a project (for people and coding agents):",
        ] {
            assert!(text.contains(s), "{s}");
        }
        assert!(
            !text.contains("Other commands:"),
            "every command has a section:\n{text}"
        );
    }
}
