use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BrowserMotionPolicy {
    Reduce,
    Default,
}

impl BrowserMotionPolicy {
    pub(crate) fn parse(value: &str) -> Result<Self, String> {
        match value {
            "reduce" => Ok(Self::Reduce),
            "default" => Ok(Self::Default),
            other => Err(format!(
                "mode must be \"reduce\" or \"default\", got {other:?}"
            )),
        }
    }

    pub(crate) fn prefers_reduced_motion(self) -> bool {
        matches!(self, Self::Reduce)
    }

    pub(crate) fn cdp_params(self) -> Value {
        match self {
            Self::Reduce => json!({
                "media": "",
                "features": [
                    {
                        "name": "prefers-reduced-motion",
                        "value": "reduce"
                    }
                ]
            }),
            // Empty features clears the emulated media-feature overrides and
            // returns the page to the browser/system default.
            Self::Default => json!({
                "media": "",
                "features": []
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduce_uses_only_the_standard_reduced_motion_media_feature() {
        let params = BrowserMotionPolicy::Reduce.cdp_params();

        assert_eq!(params["media"], "");
        assert_eq!(
            params["features"],
            json!([{
                "name": "prefers-reduced-motion",
                "value": "reduce"
            }])
        );
    }

    #[test]
    fn default_clears_media_feature_emulation() {
        let params = BrowserMotionPolicy::Default.cdp_params();

        assert_eq!(params["media"], "");
        assert_eq!(params["features"], json!([]));
    }

    #[test]
    fn parser_is_closed_and_explicit() {
        assert_eq!(
            BrowserMotionPolicy::parse("reduce").expect("reduce"),
            BrowserMotionPolicy::Reduce
        );
        assert_eq!(
            BrowserMotionPolicy::parse("default").expect("default"),
            BrowserMotionPolicy::Default
        );
        assert!(BrowserMotionPolicy::parse("disable-everything").is_err());
    }
}
