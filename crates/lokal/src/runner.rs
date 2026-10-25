//! Starting a dev server behind its route (SPEC §4.10): the `{port}`
//! placeholder and the environment a command gets. Pure functions, shared
//! by `lokal run` and, later, `lokal up`.

use std::collections::BTreeMap;

use crate::state::{self, Service};

/// The placeholder replaced with the port in every argument.
pub const PORT_PLACEHOLDER: &str = "{port}";

/// `arg` with every `{port}` replaced by `port`.
pub fn substitute_port(arg: &str, port: u16) -> String {
    arg.replace(PORT_PLACEHOLDER, &port.to_string())
}

/// `LOKAL_URL_<SUB>`: the subdomain uppercased, with `.` and `-` turned into
/// `_`. The apex `@` is `APEX`.
pub fn url_var(sub: &str) -> String {
    let name = if sub == "@" {
        "APEX".to_string()
    } else {
        sub.to_ascii_uppercase().replace(['.', '-'], "_")
    };
    format!("LOKAL_URL_{name}")
}

/// The variables added to a command's environment, and warnings to print.
///
/// `services` is the project's services after the state write (including
/// `sub`). The command gets `PORT`, `HOST` (the address to listen on:
/// `127.0.0.1` for a `localhost` route, else the route's loopback IP),
/// `LOKAL_URL` (its own URL) and `LOKAL_URL_<SUB>` for every service. When
/// two services map to the same `LOKAL_URL_<SUB>` (`v1.api` and `v1-api`,
/// or a service named `apex` next to `@`), neither is set, with one warning
/// per name.
pub fn child_env(
    domain: &str,
    services: &BTreeMap<String, Service>,
    sub: &str,
    port: u16,
) -> (Vec<(String, String)>, Vec<String>) {
    let url = |s: &str| format!("https://{}", state::fqdn(s, domain));
    let host = match services.get(sub) {
        Some(s) if s.host != "localhost" => s.host.clone(),
        _ => "127.0.0.1".to_string(),
    };
    let mut env = vec![
        ("PORT".to_string(), port.to_string()),
        ("HOST".to_string(), host),
        ("LOKAL_URL".to_string(), url(sub)),
    ];
    let mut by_var: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for s in services.keys() {
        by_var.entry(url_var(s)).or_default().push(s);
    }
    let mut warnings = vec![];
    for (var, subs) in by_var {
        match subs.as_slice() {
            [one] => env.push((var, url(one))),
            many => warnings.push(format!(
                "{var} isn't set: services {} all map to it",
                many.iter()
                    .map(|s| format!("`{s}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }
    (env, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::HostHeader;

    fn svc(port: u16) -> Service {
        Service {
            host: "localhost".into(),
            port,
            host_header: HostHeader::Preserve,
            managed: None,
        }
    }

    fn get<'a>(env: &'a [(String, String)], k: &str) -> Option<&'a str> {
        env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn port_placeholder_is_replaced_in_each_argument() {
        let args = [
            "vite",
            "--port",
            "{port}",
            "127.0.0.1:{port}",
            "{port}{port}",
            "{PORT}",
        ];
        let got: Vec<String> = args.iter().map(|a| substitute_port(a, 5173)).collect();
        assert_eq!(
            got,
            [
                "vite",
                "--port",
                "5173",
                "127.0.0.1:5173",
                "51735173",
                "{PORT}"
            ]
        );
    }

    #[test]
    fn url_variable_names() {
        assert_eq!(url_var("api"), "LOKAL_URL_API");
        assert_eq!(url_var("v1.api"), "LOKAL_URL_V1_API");
        assert_eq!(url_var("db-ui"), "LOKAL_URL_DB_UI");
        assert_eq!(url_var("@"), "LOKAL_URL_APEX");
        assert_eq!(url_var("1st"), "LOKAL_URL_1ST");
    }

    #[test]
    fn env_has_port_host_own_url_and_every_service() {
        let services = BTreeMap::from([
            ("@".to_string(), svc(3000)),
            ("v1.api".to_string(), svc(4000)),
            ("web".to_string(), svc(5173)),
        ]);
        let (env, warnings) = child_env("shop.lokal", &services, "web", 5173);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(get(&env, "PORT"), Some("5173"));
        assert_eq!(get(&env, "HOST"), Some("127.0.0.1"));
        assert_eq!(get(&env, "LOKAL_URL"), Some("https://web.shop.lokal"));
        assert_eq!(get(&env, "LOKAL_URL_WEB"), Some("https://web.shop.lokal"));
        assert_eq!(
            get(&env, "LOKAL_URL_V1_API"),
            Some("https://v1.api.shop.lokal")
        );
        assert_eq!(get(&env, "LOKAL_URL_APEX"), Some("https://shop.lokal"));
        assert_eq!(env.len(), 6);
    }

    #[test]
    fn colliding_names_set_neither_and_warn_once() {
        let services = BTreeMap::from([
            ("@".to_string(), svc(3000)),
            ("apex".to_string(), svc(3001)),
            ("v1.api".to_string(), svc(4000)),
            ("v1-api".to_string(), svc(4001)),
            ("web".to_string(), svc(5173)),
        ]);
        let (env, warnings) = child_env("shop.lokal", &services, "v1.api", 4000);
        assert_eq!(get(&env, "LOKAL_URL_V1_API"), None);
        assert_eq!(get(&env, "LOKAL_URL_APEX"), None);
        assert_eq!(get(&env, "LOKAL_URL_WEB"), Some("https://web.shop.lokal"));
        // The command's own URL is still there.
        assert_eq!(get(&env, "LOKAL_URL"), Some("https://v1.api.shop.lokal"));
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("LOKAL_URL_V1_API")
            && w.contains("`v1-api`")
            && w.contains("`v1.api`")));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("LOKAL_URL_APEX") && w.contains("`@`") && w.contains("`apex`"))
        );
    }

    #[test]
    fn host_follows_a_loopback_ip_route() {
        let mut v6 = svc(4000);
        v6.host = "::1".into();
        let services = BTreeMap::from([("api".to_string(), v6)]);
        let (env, _) = child_env("shop.lokal", &services, "api", 4000);
        assert_eq!(get(&env, "HOST"), Some("::1"));
    }
}
