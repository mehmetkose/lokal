//! systemd's socket-passing protocol (`sd_listen_fds_with_names(3)`): the
//! sockets a service gets are fds 3, 4, … with their count in `LISTEN_FDS`,
//! for the process whose pid is `LISTEN_PID`, and their names, in order, in
//! the `:`-separated `LISTEN_FDNAMES`. A pure function, compiled on every OS
//! so that `cargo test` on a Mac covers it too; only `platform/linux/` calls
//! it (VERIFY V12 on Linux, SPEC §12).

/// The first passed fd (`SD_LISTEN_FDS_START`).
pub(super) const FIRST_FD: i32 = 3;

/// The fds passed to process `pid`, each with its name, from the values of
/// `LISTEN_PID`, `LISTEN_FDS` and `LISTEN_FDNAMES`. `Ok(None)` when they
/// aren't for this process: no `LISTEN_PID`, or another pid's (a child
/// inherits them from its parent). An fd without a name is `unknown`, as
/// systemd names it.
pub(super) fn passed_fds(
    pid: u32,
    listen_pid: Option<&str>,
    listen_fds: Option<&str>,
    names: Option<&str>,
) -> Result<Option<Vec<(i32, String)>>, String> {
    if listen_pid.and_then(|p| p.trim().parse::<u32>().ok()) != Some(pid) {
        return Ok(None);
    }
    let n: i32 = listen_fds
        .and_then(|n| n.trim().parse().ok())
        .ok_or_else(|| format!("LISTEN_FDS isn't a count: {listen_fds:?}"))?;
    let names: Vec<&str> = match names {
        Some("") => vec![],
        Some(s) => s.split(':').collect(),
        None => vec!["unknown"; n.max(0) as usize],
    };
    if names.len() != n.max(0) as usize {
        return Err(format!(
            "LISTEN_FDNAMES has {} names for {n} fds",
            names.len()
        ));
    }
    Ok(Some(
        (FIRST_FD..)
            .zip(names)
            .map(|(fd, name)| (fd, name.to_string()))
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_each_passed_fd() {
        // The order across socket units changes from start to start (V12).
        assert_eq!(
            passed_fds(42, Some("42"), Some("5"), Some("https:https:dns:http:http")),
            Ok(Some(vec![
                (3, "https".into()),
                (4, "https".into()),
                (5, "dns".into()),
                (6, "http".into()),
                (7, "http".into()),
            ]))
        );
        assert_eq!(
            passed_fds(42, Some("42"), Some("2"), None),
            Ok(Some(vec![(3, "unknown".into()), (4, "unknown".into())]))
        );
        assert_eq!(
            passed_fds(42, Some("42"), Some("0"), Some("")),
            Ok(Some(vec![]))
        );
    }

    #[test]
    fn fds_for_another_process_are_not_ours() {
        assert_eq!(passed_fds(42, None, Some("5"), Some("https")), Ok(None));
        assert_eq!(
            passed_fds(42, Some("41"), Some("1"), Some("https")),
            Ok(None)
        );
        assert_eq!(
            passed_fds(42, Some("x"), Some("1"), Some("https")),
            Ok(None)
        );
    }

    #[test]
    fn a_count_that_doesnt_match_is_an_error() {
        assert!(passed_fds(42, Some("42"), None, None).is_err());
        assert!(passed_fds(42, Some("42"), Some("two"), None).is_err());
        assert!(passed_fds(42, Some("42"), Some("2"), Some("https")).is_err());
    }
}
