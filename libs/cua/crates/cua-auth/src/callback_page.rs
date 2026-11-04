//! The page the browser shows on the loopback sign-in redirect. One
//! self-contained template (inline CSS and mark, no network requests) for
//! the CLI and the apps. It never shows anything from the redirect's query
//! string: the code and state stay out of the page, and the provider's error
//! text goes to the caller instead.

const TEMPLATE: &str = include_str!("callback.html");

/// Where the person goes back to once the browser part is done; it words the
/// callback page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturnTo {
    /// `cua auth login` and other CLI commands.
    Terminal,
    /// An app, by its display name ("Cua Spaces").
    App(String),
}

impl ReturnTo {
    /// `CUA_AUTH_RETURN_TO`: `terminal`, or the display name of the app that
    /// started sign-in. Unset: neither (the page names no destination).
    pub fn from_env() -> Option<Self> {
        let v = std::env::var("CUA_AUTH_RETURN_TO").ok()?;
        match v.trim() {
            "" => None,
            "terminal" => Some(ReturnTo::Terminal),
            name => Some(ReturnTo::App(name.to_string())),
        }
    }
}

/// What the callback page reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Signed in, as this email when the tokens carry one.
    SignedIn(Option<String>),
    /// The person declined at the provider (`access_denied`).
    Cancelled,
    /// The redirect belongs to an earlier attempt (state mismatch).
    Stale,
    /// The provider would not exchange the code (it expired or was used).
    Expired,
    /// Anything else: a provider error, a missing code, a failed exchange.
    Failed,
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// The page for `outcome`.
pub(crate) fn render(outcome: &Outcome, to: Option<&ReturnTo>) -> String {
    let retry = match to {
        Some(ReturnTo::Terminal) => "To try again, run <code>cua auth login</code>.".to_string(),
        Some(ReturnTo::App(app)) => format!("To try again, sign in from {}.", escape(app)),
        None => "To try again, start sign-in again where you began.".to_string(),
    };
    let (kind, title, heading, body) = match outcome {
        Outcome::SignedIn(email) => {
            let account = email
                .as_deref()
                .filter(|e| !e.is_empty())
                .map(|e| format!("<p class=\"account\">{}</p>\n", escape(e)))
                .unwrap_or_default();
            let next = match to {
                Some(ReturnTo::Terminal) => {
                    "You can close this tab and return to your terminal.".to_string()
                }
                Some(ReturnTo::App(app)) => {
                    format!("You can close this tab and return to {}.", escape(app))
                }
                None => "You can close this tab.".to_string(),
            };
            (
                "signed-in",
                "Signed in to Cua",
                "You're signed in",
                format!("{account}<p>{next}</p>"),
            )
        }
        Outcome::Cancelled => (
            "error",
            "Sign-in cancelled - Cua",
            "Sign-in was cancelled",
            format!("<p>Cua didn't get access to your account.</p>\n<p>{retry}</p>"),
        ),
        Outcome::Stale => (
            "error",
            "Sign-in out of date - Cua",
            "This sign-in is out of date",
            format!(
                "<p>This page is from an earlier sign-in attempt, so Cua ignored it. If you started again, finish in the newest tab.</p>\n<p>{retry}</p>"
            ),
        ),
        Outcome::Expired => (
            "error",
            "Sign-in expired - Cua",
            "This sign-in expired",
            format!(
                "<p>It took too long to finish, or the link was already used.</p>\n<p>{retry}</p>"
            ),
        ),
        Outcome::Failed => {
            let details = match to {
                Some(ReturnTo::Terminal) => " The details are in your terminal.".to_string(),
                Some(ReturnTo::App(app)) => format!(" The details are in {}.", escape(app)),
                None => String::new(),
            };
            (
                "error",
                "Sign-in failed - Cua",
                "Sign-in didn't finish",
                format!("<p>Cua couldn't complete the sign-in.{details}</p>\n<p>{retry}</p>"),
            )
        }
    };
    // Browsers only let a script close a tab a script opened; elsewhere this
    // does nothing and the page stays.
    let script = if kind == "signed-in" {
        "<script>setTimeout(function () { window.close(); }, 1500);</script>"
    } else {
        ""
    };
    TEMPLATE
        .replace("{{title}}", title)
        .replace("{{kind}}", kind)
        .replace("{{heading}}", heading)
        .replace("{{script}}", script)
        .replace("{{body}}", &body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all(email: Option<&str>) -> Vec<Outcome> {
        vec![
            Outcome::SignedIn(email.map(String::from)),
            Outcome::Cancelled,
            Outcome::Stale,
            Outcome::Expired,
            Outcome::Failed,
        ]
    }

    #[test]
    fn every_page_is_self_contained_and_filled_in() {
        for to in [
            None,
            Some(ReturnTo::Terminal),
            Some(ReturnTo::App("Cua Spaces".into())),
        ] {
            for o in all(Some("t@example.com")) {
                let p = render(&o, to.as_ref());
                assert!(p.starts_with("<!doctype html>"), "{o:?}");
                assert!(!p.contains("{{"), "unfilled placeholder in {o:?}");
                assert!(!p.contains("http://") && !p.contains("https://"), "{o:?}");
                assert!(p.contains("prefers-reduced-motion"));
            }
        }
    }

    #[test]
    fn words_the_destination() {
        let t = Some(ReturnTo::Terminal);
        let app = Some(ReturnTo::App("Cua Spaces".into()));
        let ok = Outcome::SignedIn(Some("t@example.com".into()));
        let p = render(&ok, t.as_ref());
        assert!(p.contains("You're signed in") && p.contains("t@example.com"));
        assert!(p.contains("return to your terminal") && p.contains("window.close()"));
        assert!(render(&ok, app.as_ref()).contains("return to Cua Spaces"));
        let p = render(&Outcome::Cancelled, t.as_ref());
        assert!(p.contains("<code>cua auth login</code>") && !p.contains("window.close()"));
        assert!(render(&Outcome::Expired, app.as_ref()).contains("sign in from Cua Spaces"));
    }

    #[test]
    fn escapes_the_email_and_app_name() {
        let to = ReturnTo::App("<b>".into());
        let p = render(&Outcome::SignedIn(Some("<x>@e.com".into())), Some(&to));
        assert!(p.contains("&lt;x&gt;@e.com") && p.contains("return to &lt;b&gt;."));
        assert!(!p.contains("<x>") && !p.contains("<b>"));
    }
}
