//! DNN's install page as it streams in (`src/DnnManager.Infrastructure/Dnn/DnnInstallOutput.cs`): one line per step,
//! flushed as DNN goes, ending in "Installation Complete". Turns it into readable steps, and decides how the install
//! went - from what DNN wrote, not from the HTTP status, which is 200 even when the install failed.

use std::sync::LazyLock;

use regex::Regex;

/// One step of DNN's install output, e.g. "Installing Package File DNNCE_HTML_10.03.03_Install" - and whether DNN marked
/// it Success or Error!.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DnnInstallStep {
    pub text: String,
    pub succeeded: Option<bool>,
}

/// Any of these means the install didn't fully succeed, even when "Installation Complete" follows.
pub const FAILURE_MARKERS: [&str; 4] = ["Error!", "Upgrade Error", "failed to install", "currently in progress"];

static DELIMITER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>|</h[1-6]>|\n").expect("valid"));
static TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new("<[^>]+>").expect("valid"));
static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").expect("valid"));
// DNN puts its own elapsed time first: "00:00:12.345 - Installing …".
static ELAPSED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\d{2}:\d{2}:\d{2}(\.\d{1,7})?\s*-\s*").expect("valid"));
static STATUS_FONT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<font color='(green|red)'>\s*(Success|Error!)\s*</font>").expect("valid"));
// ASP.NET's error page: "<b> Exception Details: </b>System.IO.IOException: …<br><br>".
static EXCEPTION_DETAILS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)Exception Details:\s*</b>(.*?)<br").expect("valid"));

fn contains_ignore_case(text: &str, part: &str) -> bool {
    text.to_lowercase().contains(&part.to_lowercase())
}

fn starts_with_ignore_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

/// `WebUtility.HtmlDecode` for what DNN and ASP.NET write: named and numeric character references.
pub fn html_decode(text: &str) -> String {
    if !text.contains('&') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let decoded = rest.find(';').filter(|end| *end <= 10).and_then(|end| {
            let entity = &rest[1..end];
            let c = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some('\u{a0}'),
                _ if entity.starts_with("#x") || entity.starts_with("#X") => {
                    u32::from_str_radix(&entity[2..], 16).ok().and_then(char::from_u32)
                }
                _ if entity.starts_with('#') => entity[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            }?;
            Some((c, end))
        });
        match decoded {
            Some((c, end)) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// DNN's install or upgrade page, read as it streams in.
#[derive(Debug, Default)]
pub struct DnnInstallOutput {
    body: String,
    pending: String,
    failed_steps: Vec<DnnInstallStep>,
    // The page header (styles, logo) comes before DNN's first <h1>; a comment around it hides part of it.
    started: bool,
    in_comment: bool,
    server_error: bool,
    last: Option<DnnInstallStep>,
}

impl DnnInstallOutput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything DNN sent so far.
    pub fn body(&self) -> &str {
        &self.body
    }

    /// The exception of ASP.NET's error page when DNN's page broke off into one - None when it didn't.
    pub fn server_error(&self) -> Option<String> {
        let found = EXCEPTION_DETAILS.captures(&self.body)?;
        let text = html_decode(&TAG.replace_all(&found[1], " "));
        let text = text.trim();
        (!text.is_empty()).then(|| WHITESPACE.replace_all(text, " ").into_owned())
    }

    /// The last step DNN reported before its page ended - where it stopped when it didn't finish.
    pub fn last_step(&self) -> Option<&str> {
        self.last.as_ref().map(|s| s.text.as_str())
    }

    /// Takes the next chunk of the page and returns the steps it completed.
    pub fn add(&mut self, chunk: &str) -> Vec<DnnInstallStep> {
        self.body.push_str(chunk);
        self.pending.push_str(chunk);
        let text = std::mem::take(&mut self.pending);
        let mut steps = Vec::new();
        let mut start = 0;
        let ends: Vec<usize> = DELIMITER.find_iter(&text).map(|m| m.end()).collect();
        for end in ends {
            if let Some(step) = self.parse(&text[start..end]) {
                steps.push(step);
            }
            start = end;
        }
        self.pending = text[start..].to_string();
        steps
    }

    /// What is left once DNN has closed the response.
    pub fn finish(&mut self) -> Vec<DnnInstallStep> {
        let rest = std::mem::take(&mut self.pending);
        self.parse(&rest).into_iter().collect()
    }

    fn markers(&self) -> Vec<&'static str> {
        FAILURE_MARKERS.into_iter().filter(|m| contains_ignore_case(&self.body, m)).collect()
    }

    fn failed(&self) -> Vec<&str> {
        self.failed_steps.iter().take(3).map(|s| s.text.as_str()).collect()
    }

    /// Ok when DNN wrote "Installation Complete" and "Successfully Installed Site 0" and nothing it marks as an error;
    /// otherwise what went wrong, as DNN put it.
    pub fn outcome(&self) -> Result<(), String> {
        let html = &self.body;
        let markers = self.markers();
        let complete = html.contains("Installation Complete");
        let site = html.contains("Successfully Installed Site 0");
        if complete && site && markers.is_empty() {
            return Ok(());
        }
        if html.contains("Nothing To Install At This Time") || html.contains("current Database Version are identical") {
            return Err("DNN is installed already - its database holds a DNN site.".into());
        }
        if markers.contains(&"currently in progress") {
            return Err("Another installation of this site is running (DNN's installBlocker.lock).".into());
        }
        if contains_ignore_case(html, "Could not connect to database") {
            return Err("DNN couldn't connect to its database - check the site's connection string and that the site's identity may sign in.".into());
        }
        let failed = self.failed();
        let why = if !failed.is_empty() {
            failed.join("; ")
        } else if !complete {
            "it stopped before \"Installation Complete\"".into()
        } else if !site {
            "the website wasn't created".into()
        } else {
            format!("it reported {}", markers.join(", "))
        };
        Err(format!("DNN's installation didn't complete: {why}."))
    }

    /// For `Install.aspx?mode=upgrade`: Ok when DNN wrote "Upgrade Complete" and nothing it marks as an error - or that
    /// there was nothing to upgrade; otherwise what went wrong, as DNN put it.
    pub fn upgrade_outcome(&self) -> Result<(), String> {
        let html = &self.body;
        // DNN broke off into ASP.NET's error page before it finished: where it stopped, and what the page says.
        if let Some(crashed) = self.server_error().filter(|_| !contains_ignore_case(html, "Upgrade Complete")) {
            return Err(format!(
                "DNN's upgrade stopped{} and broke off with: {crashed}{}",
                self.last_step().map(|last| format!(" at \"{last}\"")).unwrap_or_default(),
                if contains_ignore_case(&crashed, "installBlocker") {
                    " - an error DNN hits cleaning up after a failed upgrade, which hides the real one: DNN's logs and the database scripts' logs say what failed."
                } else {
                    ""
                }
            ));
        }
        let markers = self.markers();
        if markers.contains(&"currently in progress") {
            return Err("Another installation or upgrade of this site is running (DNN's installBlocker.lock).".into());
        }
        if contains_ignore_case(html, "Could not connect to database") {
            return Err("DNN couldn't connect to its database - check the site's connection string and that the site's identity may sign in.".into());
        }
        if markers.is_empty()
            && (contains_ignore_case(html, "Upgrade Complete")
                || contains_ignore_case(html, "current Database Version are identical")
                || contains_ignore_case(html, "Nothing To Upgrade"))
        {
            return Ok(());
        }
        let failed = self.failed();
        let why = if !failed.is_empty() {
            failed.join("; ")
        } else if !markers.is_empty() {
            format!("it reported {}", markers.join(", "))
        } else {
            "it stopped before \"Upgrade Complete\"".into()
        };
        Err(format!("DNN's upgrade didn't complete: {why}."))
    }

    fn parse(&mut self, segment: &str) -> Option<DnnInstallStep> {
        let mut s = segment.to_string();
        if self.in_comment {
            let close = s.find("-->")?;
            self.in_comment = false;
            s = s[close + 3..].to_string();
        }
        if let Some(open) = s.find("<!--") {
            match s[open..].find("-->").map(|c| c + open) {
                None => {
                    self.in_comment = true;
                    s.truncate(open);
                }
                Some(close) => s = format!("{}{}", &s[..open], &s[close + 3..]),
            }
        }
        if !self.started {
            if !contains_ignore_case(&s, "<h1>") {
                return None;
            }
            self.started = true;
        }
        // ASP.NET's error page after DNN's own output: not a step - its exception is read as a whole.
        if self.server_error {
            return None;
        }
        if contains_ignore_case(&s, "Server Error in")
            || contains_ignore_case(&s, "<!DOCTYPE")
            || contains_ignore_case(&s, "<title>")
        {
            self.server_error = true;
            return None;
        }

        let mut succeeded = None;
        if let Some(status) = STATUS_FONT.captures(&s) {
            succeeded = Some(starts_with_ignore_case(&status[2], "Success"));
            s = STATUS_FONT.replace_all(&s, " ").into_owned();
        }
        let decoded = html_decode(&TAG.replace_all(&s, " "));
        let mut text = WHITESPACE.replace_all(&decoded, " ").trim().to_string();
        if let Some(elapsed) = ELAPSED.find(&text) {
            text = text[elapsed.end()..].trim().to_string();
        }
        let mut text = text.trim_end_matches(':').trim().to_string();
        if text.is_empty() && succeeded.is_none() {
            return None;
        }
        if starts_with_ignore_case(&text, "Click Here To Access Your Site") {
            return None;
        }
        if text.encode_utf16().count() > 300 {
            text = format!("{}…", text.chars().take(300).collect::<String>());
        }
        // "Site failed to install:Error!" and "Upgrade Error: …" carry no Success/Error font of their own.
        if succeeded.is_none()
            && (contains_ignore_case(&text, "failed to install") || starts_with_ignore_case(&text, "Upgrade Error"))
        {
            succeeded = Some(false);
        }
        let step = DnnInstallStep { text, succeeded };
        if succeeded == Some(false) {
            self.failed_steps.push(step.clone());
        }
        self.last = Some(step.clone());
        Some(step)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // What DNN 10.3.3's Install.aspx streamed in a real install (shortened), and the end of one with a too-short host
    // password.
    const SUCCEEDED: &str = concat!(
        "<html><head><title>DotNetNuke</title></head><body><img src=\"../images/branding/DNN_logo.png\"><br>\r\n",
        "<!-- tags excluded on purpose\r\n</body>\r\n</html>\r\n-->\r\n<h1>Installing DNN</h1>",
        "00:00:00.025 -&nbsp;Checking File and Folder permissions <font color='green'>Success</font><br>",
        "00:00:00.083 -&nbsp;&nbsp;&nbsp;Executing Script:DotNetNuke.Schema.SqlDataProvider&nbsp;<font color='green'>Success</font><br>",
        "00:00:09.121 -&nbsp;&nbsp;&nbsp;Installing Package File DNNCE_HTML_10.03.03_Install: &nbsp;<font color='green'>Success</font><br>",
        "00:00:16.559 -&nbsp;&nbsp;&nbsp;Creating Site: My &amp; Site<br>",
        "00:00:18.904 -&nbsp;&nbsp;&nbsp;Creating Site Alias: localhost:8081<br>",
        "00:00:18.910 -&nbsp;&nbsp;&nbsp;<font color='green'>Successfully Installed Site 0:</font><br>",
        "<h2>Installation Complete</h2><br><br><h2><a href='../Default.aspx'>Click Here To Access Your Site</a></h2><br><br></body></html>"
    );

    const SHORT_PASSWORD: &str = concat!(
        "<h1>Installing DNN</h1>",
        "00:00:16.559 -&nbsp;&nbsp;&nbsp;Creating Site: ShortPw<br>",
        "00:00:18.904 -&nbsp;&nbsp;&nbsp;Creating Site Alias: localhost:8085<br>",
        "00:00:18.950 -&nbsp;Object reference not set to an instance of an object. at DotNetNuke.Services.Upgrade.Upgrade.AddPortal <font color='red'>Error!</font><br>",
        "Site failed to install:<font color='red'>Error!</font><br>",
        "<h2>Installation Complete</h2><br>"
    );

    const PACKAGE_FAILED: &str = concat!(
        "<h1>Installing DNN</h1>",
        "00:00:12.256 -&nbsp;&nbsp;&nbsp;Installing Package File AspNetClientCapabilityProvider_10.03.03_Install: &nbsp;<font color='red'>Error!</font><br>",
        "00:00:18.910 -&nbsp;&nbsp;&nbsp;<font color='green'>Successfully Installed Site 0:</font><br>",
        "<h2>Installation Complete</h2><br>"
    );

    fn read(page: &str) -> DnnInstallOutput {
        let mut output = DnnInstallOutput::new();
        output.add(page);
        output.finish();
        output
    }

    #[test]
    fn a_clean_install_succeeds_in_readable_steps() {
        let mut output = DnnInstallOutput::new();
        // In small pieces, as it streams in.
        let mut steps = Vec::new();
        let chars: Vec<char> = SUCCEEDED.chars().collect();
        for piece in chars.chunks(37) {
            steps.extend(output.add(&piece.iter().collect::<String>()));
        }
        steps.extend(output.finish());
        assert_eq!(output.outcome(), Ok(()));
        let texts: Vec<_> = steps.iter().map(|s| s.text.as_str()).collect();
        assert!(texts.contains(&"Installing Package File DNNCE_HTML_10.03.03_Install"), "{texts:?}");
        assert!(texts.contains(&"Creating Site: My & Site"), "{texts:?}");
        assert_eq!(steps.iter().find(|s| s.text.starts_with("Checking File")).unwrap().succeeded, Some(true));
        assert!(!texts.iter().any(|t| t.contains("DotNetNuke</title") || t.contains("Click Here")));
    }

    #[test]
    fn a_short_password_fails_although_installation_complete() {
        assert!(read(SHORT_PASSWORD).outcome().unwrap_err().contains("Object reference not set"));
    }

    #[test]
    fn a_failed_package_fails_although_the_site_was_installed() {
        assert!(read(PACKAGE_FAILED).outcome().unwrap_err().contains("AspNetClientCapabilityProvider"));
    }

    #[test]
    fn already_installed_says_so() {
        let output = read("<h1>Upgrade Status Report</h1>Nothing To Install At This Time<br>");
        assert!(output.outcome().unwrap_err().contains("installed already"));
    }

    #[test]
    fn an_upgrade_that_broke_off_into_aspnets_error_page_says_where_and_why() {
        let output = read(concat!(
            "<h1>Upgrade Status Report</h1>00:00:01.000 - Upgrading DNN<br>00:00:02.000 - Executing Script: 10.03.00<br>",
            "<!DOCTYPE html><html><head><title>The process cannot access the file</title></head><body>",
            "<H1>Server Error in '/' Application.<hr></H1><b> Exception Details: </b>System.IO.IOException: The process cannot ",
            "access the file 'C:\\DNN\\shop\\installBlocker.lock'.<br><br></body></html>"
        ));
        let error = output.upgrade_outcome().unwrap_err();
        assert!(
            error.starts_with(
                "DNN's upgrade stopped at \"Executing Script: 10.03.00\" and broke off with: System.IO.IOException"
            ),
            "{error}"
        );
        assert!(error.ends_with("say what failed."), "{error}");
        assert_eq!(read("<h1>x</h1>00:00:59.000 - Upgrade Complete<br>").upgrade_outcome(), Ok(()));
        assert_eq!(
            read("<h1>x</h1>Upgrading<br>").upgrade_outcome().unwrap_err(),
            "DNN's upgrade didn't complete: it stopped before \"Upgrade Complete\"."
        );
    }

    #[test]
    fn html_is_decoded() {
        assert_eq!(html_decode("a &amp; b &lt;c&gt; &#39;d&#x27; &bogus; &"), "a & b <c> 'd' &bogus; &");
    }
}
