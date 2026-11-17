//! Why an upgrade step most likely failed, and what to do about it
//! (`src/DnnManager.Application/Upgrades/DnnUpgradeDiagnosis.cs`).

/// What most likely made an upgrade step fail, and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeDiagnosis {
    pub cause: String,
    pub fixes: Vec<String>,
}

/// A cause; `weak`: one that says little on its own (a time-out, an HTTP 503) - what the logs name comes first.
struct Rule {
    fits: fn(&str) -> bool,
    cause: &'static str,
    fixes: &'static [&'static str],
    weak: bool,
}

fn has(text: &str, part: &str) -> bool {
    text.to_lowercase().contains(&part.to_lowercase())
}

const RULES: &[Rule] = &[
    Rule {
        fits: |t| has(t, "Could not load file or assembly") || has(t, "manifest definition does not match"),
        cause: "An assembly in bin doesn't match web.config's binding redirect for it - the site can't start with the new files (DNN issues #6782, #6993).",
        fixes: &[
            "Compare web.config's <runtime><assemblyBinding> redirect for the assembly named with the version of the DLL in bin.",
            "From DNN 10.2 on, upgrade with DNN's own local upgrade (DNN Manager does so) instead of unzipping the upgrade package.",
        ],
        weak: false,
    },
    // Not just a script's log named among those kept: a script that failed, or SQL that DNN logged as failing.
    Rule {
        fits: |t| {
            has(t, "Database script")
                || has(t, "Error executing SQL")
                || has(t, "SqlDataProvider") && has(t, "stopped at")
        },
        cause: "One of DNN's database scripts failed.",
        fixes: &[
            "The DNN log line above (kept in the step's failed-upgrade folder) names the SQL that failed and SQL Server's error; a script's own log in Providers\\DataProviders\\SqlDataProvider may add to it.",
            "Third-party objects on DNN's tables (triggers, views, constraints) often make them fail - fix or remove the one named, then upgrade again.",
        ],
        weak: false,
    },
    Rule {
        fits: |t| {
            has(t, "installBlocker")
                || has(t, "currently in progress")
                || has(t, "installation/upgrade was in progress")
        },
        cause: "DNN thinks an installation or upgrade is still running: its installBlocker.lock is there. DNN keeps the lock open until its worker process ends, so it can't delete it itself at the end of its upgrade.",
        fixes: &[
            "Stop the site, wait for its worker process (w3wp.exe) to end - or end it in Task Manager - then delete installBlocker.lock in the site's folder and start the site.",
            "Make sure nobody runs DNN's installer meanwhile.",
        ],
        weak: false,
    },
    Rule {
        fits: |t| {
            has(t, "Could not connect to database")
                || has(t, "couldn't connect to its database")
                || has(t, "Login failed")
                || has(t, "A network-related or instance-specific error")
                || has(t, "The database can't be read")
        },
        cause: "The site can't reach its database.",
        fixes: &[
            "Check that SQL Server runs and that the site's login (web.config's SiteSqlServer, or the app pool's identity) may sign in and owns the database.",
        ],
        weak: false,
    },
    Rule {
        fits: |t| has(t, "-5324627661") || has(t, "CodeDom") || has(t, "compiler"),
        cause: "The C# compiler package (CodeDom) changed and ASP.NET can't compile the site (DNN issue #6448).",
        fixes: &[
            "Uninstall the Microsoft.CodeDom.Providers.DotNetCompilerPlatform 3.6 extension before upgrading, or remove web.config's <system.codedom><compilers>.",
        ],
        weak: false,
    },
    Rule {
        fits: |t| has(t, "Telerik"),
        cause: "Something still needs Telerik, which DNN 10 removes.",
        fixes: &[
            "Update or remove the extensions that use Telerik (DNN 9.8+: Settings → Servers → Telerik removal lists them), then upgrade again.",
        ],
        weak: false,
    },
    Rule {
        fits: |t| has(t, "Content was lost"),
        cause: "The upgrade changed the site's content - portals, users, roles, pages or permissions are fewer than before.",
        fixes: &[
            "Keep the site as the stage's backup has it (DNN Manager puts it back) and look at what the upgrade's database scripts did before trying again.",
        ],
        weak: false,
    },
    Rule {
        fits: |t| has(t, "can't sign in") || has(t, "Persona Bar"),
        cause: "The site runs, but signing in or DNN's Persona Bar doesn't work after the upgrade.",
        fixes: &[
            "Look at DNN's log (kept with the stage's diagnostics) for errors from the Persona Bar or the membership provider; clearing the site's cache and recycling the app pool often helps.",
        ],
        weak: false,
    },
    Rule {
        fits: |t| has(t, "HTTP 503") || has(t, "Service Unavailable"),
        cause: "The site doesn't answer (HTTP 503): its app pool stopped - IIS's rapid-fail protection after crashes - or DNN's lock is still there.",
        fixes: &[
            "Read the Windows events kept with the stage's diagnostics (WAS, ASP.NET) for why the worker process stopped.",
        ],
        weak: true,
    },
    Rule {
        fits: |t| has(t, "HTTP 500") || has(t, "error page") || has(t, "module error"),
        cause: "The site fails to show its pages after the upgrade.",
        fixes: &[
            "The ASP.NET error and DNN's log (kept with the stage's diagnostics) name the exception; an extension built for an older DNN is the usual reason.",
        ],
        weak: false,
    },
    Rule {
        fits: |t| has(t, "took longer than") || has(t, "stopped reporting progress") || has(t, "didn't answer within"),
        cause: "DNN's upgrade took too long or hung.",
        fixes: &[
            "A large database or a locked file (antivirus, an editor) slows it down - try again with nothing else using the site's folder.",
        ],
        weak: true,
    },
    Rule {
        fits: |t| has(t, "Expected DNN") || has(t, "the database at"),
        cause: "DNN's upgrade didn't run or didn't finish: the database isn't at the version of the files.",
        fixes: &["DNN's output (kept with the stage's diagnostics) shows where it stopped."],
        weak: false,
    },
];

/// The most likely cause of `error`, read together with `evidence` (logs, check results).
pub fn explain(error: &str, evidence: &[String]) -> UpgradeDiagnosis {
    // What the step failed with first - then what the logs and the checks say; a time-out or an HTTP 503 only when
    // they name nothing more telling (the site timed out because DNN's lock was left, say).
    let all = evidence.join("\n");
    for (text, weak) in [(error, false), (all.as_str(), false), (error, true), (all.as_str(), true)] {
        if let Some(rule) = RULES.iter().filter(|r| r.weak == weak).find(|r| (r.fits)(text)) {
            return UpgradeDiagnosis {
                cause: rule.cause.into(),
                fixes: rule.fixes.iter().map(|f| f.to_string()).collect(),
            };
        }
    }
    UpgradeDiagnosis {
        cause: "Unknown - none of the known problems fits.".into(),
        fixes: vec![
            "Read DNN's output, its logs and the Windows events kept with the stage's diagnostics.".into(),
            "Ask in DNN's community forums with them.".into(),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_is_read_for_its_likely_cause() {
        for (error, cause) in [
            (
                "Could not load file or assembly 'AngleSharp' or one of its dependencies. The located assembly's manifest definition does not match",
                "binding redirect",
            ),
            (
                "Another installation or upgrade of this site is running (DNN's installBlocker.lock).",
                "installBlocker.lock",
            ),
            ("DNN couldn't connect to its database - check the site's connection string", "can't reach its database"),
            ("Content was lost (Users 12 → 11).", "content"),
            ("The host 'dnnhost' can't sign in (auth cookie not set)", "signing in"),
            ("DNN's installer answered HTTP 503 Service Unavailable", "HTTP 503"),
            ("Expected DNN 10.3.3 - the files are at 10.3.3, the database at 10.2.5", "database isn't at the version"),
        ] {
            assert!(explain(error, &[]).cause.contains(cause), "{error}");
        }
    }

    #[test]
    fn a_time_out_with_dnns_lock_in_its_log_is_read_as_the_lock() {
        let evidence = vec![
            r"Kept Providers\DataProviders\SqlDataProvider\10.03.00.log.resources".to_string(),
            "DNN log: 2026-10-05 00:15:10.231+02:00 [Albadit][D:2][T:9][ERROR] DotNetNuke.Services.Exceptions.Exceptions - Resource Not Found: - / → System.Web.HttpException (0x80004005): The site was accessed while an installation/upgrade was in progress".to_string(),
        ];
        assert!(explain("The site didn't answer within 3 minutes.", &evidence).cause.contains("installBlocker.lock"));
        // With nothing more telling, a time-out is a time-out.
        assert!(explain("The site didn't answer within 3 minutes.", &evidence[..1]).cause.contains("took too long"));
    }

    #[test]
    fn a_cause_is_also_found_in_the_logs_and_an_unknown_one_says_so() {
        let from_logs = explain(
            "Step failed.",
            &["Database script 10.03.00.SqlDataProvider: Error in UpdateDatabaseVersion".into()],
        );
        assert!(from_logs.cause.contains("database scripts failed"));
        assert!(!from_logs.fixes.is_empty());
        assert!(explain("Something new.", &[]).cause.starts_with("Unknown"));
    }
}
