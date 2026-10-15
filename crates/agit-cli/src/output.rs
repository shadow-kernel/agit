use agit_core::Error;
use serde::Serialize;

pub fn emit<T: Serialize>(json: bool, result: &T, human: impl FnOnce()) {
    if json {
        let env = serde_json::json!({ "ok": true, "result": result });
        println!("{}", serde_json::to_string(&env).unwrap());
    } else {
        human();
    }
}

pub fn report_error(e: &anyhow::Error, json: bool) -> u8 {
    let details = e.downcast_ref::<Error>().and_then(|c| c.details());
    let (code, message, retryable, exit) = match e.downcast_ref::<Error>() {
        Some(core) => {
            let exit = if core.is_retryable() {
                75
            } else {
                match core {
                    Error::MergeConflicts(_)
                    | Error::LeaseConflict { .. }
                    | Error::NonFastForward { .. }
                    | Error::DirtyWorktree(_) => 3,
                    _ => 1,
                }
            };
            (core.code(), core.to_string(), core.is_retryable(), exit)
        }
        None => ("error", format!("{e:#}"), false, 1),
    };
    if json {
        let env = serde_json::json!({
            "ok": false,
            "error": { "code": code, "message": message, "retryable": retryable, "details": details }
        });
        println!("{}", serde_json::to_string(&env).unwrap());
    } else {
        eprintln!("error: {message}");
        if retryable {
            eprintln!("hint: this error is transient; retry the command");
        }
    }
    exit
}

pub fn fmt_time(ms: i64) -> String {
    // ISO-8601 UTC without pulling in a date crate.
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mth = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mth <= 2 { y + 1 } else { y };
    format!("{y:04}-{mth:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

pub fn parse_duration(s: &str) -> anyhow::Result<std::time::Duration> {
    let s = s.trim();
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len()));
    let n: u64 = num
        .parse()
        .map_err(|_| anyhow::anyhow!("invalid duration {s:?}"))?;
    let mult = match unit.trim() {
        "" | "s" | "sec" => 1,
        "m" | "min" => 60,
        "h" | "hr" => 3600,
        "d" => 86_400,
        other => anyhow::bail!("invalid duration unit {other:?} (use s, m, h, d)"),
    };
    Ok(std::time::Duration::from_secs(n * mult))
}
