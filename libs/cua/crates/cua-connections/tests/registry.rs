//! The registry's rules: what is up front, what stays hidden, who receives
//! what, and the plain-words rules for UI text.

use std::collections::BTreeMap;

use cua_connections::{
    Connected, HARNESS_IDS, MethodKind, PROVIDERS, Tier, deliver, endpoint_for, hermes, provider,
    reads, visible, works_with,
};

fn saved(all: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let m: BTreeMap<String, String> = all.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    move |n: &str| m.get(n).cloned()
}

#[test]
fn eight_providers_are_up_front_in_the_decided_order() {
    let top: Vec<&str> = visible().filter(|p| p.tier == Tier::Top).map(|p| p.id).collect();
    assert_eq!(
        top,
        ["anthropic", "openai", "openrouter", "google", "xai", "deepseek", "kimi", "custom"]
    );
}

#[test]
fn unreviewed_logins_and_unverified_providers_are_out_of_every_list() {
    for id in ["nous", "qwen-oauth", "minimax-oauth", "xai-oauth"] {
        let p = provider(id).unwrap();
        assert!(p.terms_review && p.hidden, "{id}");
        assert!(!p.visible(), "{id}");
    }
    for p in PROVIDERS.iter().filter(|p| !p.visible()) {
        assert!(!p.hidden_reason.is_empty(), "{}: says why it is hidden", p.id);
        // What a hidden provider saves is not an app's variable, nor read from a shell.
        for h in HARNESS_IDS {
            for e in p.delivery_for(h) {
                assert!(!reads(h).contains(&e.dest) || visible().any(|v| v.delivery_for(h).iter().any(|x| x.dest == e.dest)), "{} leaks {} to {h}", p.id, e.dest);
            }
        }
        for e in p.envs() {
            assert!(!cua_connections::shell_names().contains(&e) || visible().any(|v| v.envs().any(|x| x == e)), "{}: {e} readable from a shell", p.id);
            assert!(cua_connections::provider_of_env(e).is_none_or(|v| v.id != p.id), "{}", p.id);
        }
    }
    // A subscription login with unchecked terms is never offered even on a visible provider.
    for p in visible() {
        assert!(p.offered().all(|m| !m.terms_review), "{}", p.id);
    }
}

#[test]
fn what_each_app_reads_is_unchanged_for_the_migrated_providers() {
    let set = |h: &str| -> Vec<&str> {
        let mut v = reads(h).to_vec();
        v.sort_unstable();
        v
    };
    assert_eq!(set("claude-code"), ["ANTHROPIC_API_KEY", "ANTHROPIC_AUTH_TOKEN", "CLAUDE_CODE_OAUTH_TOKEN"]);
    assert_eq!(set("openai-codex"), ["CODEX_API_KEY", "CODEX_AUTH_JSON", "OPENAI_API_KEY"]);
    assert_eq!(set("gemini-cli"), ["GEMINI_API_KEY", "GOOGLE_API_KEY"]);
    assert_eq!(set("google-antigravity"), ["GEMINI_API_KEY"]);
    assert_eq!(set("opencode"), ["ANTHROPIC_API_KEY", "GEMINI_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY"]);
    assert_eq!(set("pi"), ["ANTHROPIC_API_KEY", "GEMINI_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY"]);
    assert_eq!(set("goose"), ["ANTHROPIC_API_KEY", "GOOGLE_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY"]);
    assert_eq!(set("openclaw"), ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY"]);
    // Hermes reads every key of a provider it supports, and never a plan sign-in.
    let hermes = reads("hermes");
    for must in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "OPENROUTER_API_KEY", "DEEPSEEK_API_KEY", "XAI_API_KEY", "KIMI_API_KEY", "GEMINI_API_KEY", "HF_TOKEN", "GLM_API_KEY", "FIREWORKS_API_KEY", "CUSTOM_ENDPOINT_API_KEY"] {
        assert!(hermes.contains(&must), "Hermes reads {must}");
    }
    assert!(!hermes.contains(&"CLAUDE_CODE_OAUTH_TOKEN") && !hermes.contains(&"CODEX_AUTH_JSON"));
}

/// A credential that belongs to one app never reaches another, for every provider.
#[test]
fn a_provider_credential_never_reaches_an_app_that_cannot_use_it() {
    for p in PROVIDERS {
        let mut all: Vec<(&str, String)> = vec![];
        for e in p.envs() {
            all.push((e, format!("secret-{e}")));
        }
        let pairs: Vec<(&str, &str)> = all.iter().map(|(k, v)| (*k, v.as_str())).collect();
        for h in HARNESS_IDS {
            let got = deliver(h, &saved(&pairs));
            for (dest, value) in &got {
                // Every delivered value came from this provider's own saved names.
                let src = all.iter().find(|(_, v)| v == value).map(|(k, _)| *k).unwrap();
                assert!(p.visible(), "{} is hidden and still delivers {dest} to {h}", p.id);
                if let Some(scope) = p.scope_of(src) {
                    assert_eq!(scope, *h, "{}: {src} is the {scope} sign-in and reached {h}", p.id);
                }
                // And the app reads that name.
                assert!(reads(h).contains(&dest.as_str()), "{}: {h} got {dest}, which it does not read", p.id);
            }
            // An app a provider lists no delivery for gets nothing from it.
            if p.delivery_for(h).is_empty() {
                assert!(got.is_empty(), "{} reached {h} without a delivery entry", p.id);
            }
        }
    }
}

#[test]
fn the_claude_sign_in_goes_to_claude_code_only_and_the_chatgpt_one_to_codex_only() {
    let s = saved(&[("CLAUDE_CODE_OAUTH_TOKEN", "claude"), ("CODEX_AUTH_JSON", "codex"), ("ANTHROPIC_API_KEY", "key")]);
    for h in HARNESS_IDS {
        let got = deliver(h, &s);
        assert_eq!(got.contains_key("CLAUDE_CODE_OAUTH_TOKEN"), *h == "claude-code", "{h}");
        assert_eq!(got.contains_key("CODEX_AUTH_JSON"), *h == "openai-codex", "{h}");
    }
    // Hermes gets the API key only.
    assert_eq!(deliver("hermes", &s).keys().collect::<Vec<_>>(), ["ANTHROPIC_API_KEY"]);
}

#[test]
fn a_file_that_leaks_a_scoped_sign_in_is_refused() {
    use cua_connections::schema::{ProviderDef, check};
    let toml = r#"
id = "x"
name = "X"
tier = "more"
rank = 1
kind = "hosted"
wire = "openai-chat"
base_url = "https://x.example/v1"
key_help = "h"
[[method]]
id = "login"
kind = "import_login"
env = "X_LOGIN"
scope = "claude-code"
app = "claude-code"
[delivery]
hermes = ["X_LOGIN"]
"#;
    let def: ProviderDef = ::toml::from_str(toml).unwrap();
    let err = check(&[def]).unwrap_err();
    assert!(err.contains("may only go to claude-code, not hermes"), "{err}");
}

#[test]
fn works_with_chips_are_computed_from_delivery_and_wires() {
    let w = |id: &str| works_with(provider(id).unwrap());
    assert_eq!(w("anthropic"), ["claude-code", "opencode", "goose", "pi", "hermes", "openclaw"]);
    assert_eq!(w("openai"), ["openai-codex", "opencode", "goose", "pi", "hermes", "openclaw"]);
    assert_eq!(w("google"), ["gemini-cli", "google-antigravity", "opencode", "goose", "pi", "hermes"]);
    assert_eq!(w("openrouter"), ["opencode", "goose", "pi", "hermes", "openclaw"]);
    // An app that can be pointed at the provider's wire counts, with a base URL and a model.
    assert_eq!(w("deepseek"), ["opencode", "goose", "pi", "hermes", "openclaw"]);
    assert_eq!(w("xai"), ["openai-codex", "opencode", "goose", "pi", "hermes", "openclaw"]);
    // Claude Code takes Anthropic only.
    for p in visible() {
        assert_eq!(w(p.id).contains(&"claude-code"), p.id == "anthropic", "{}", p.id);
    }
    // Hermes takes any provider that names a Hermes id and a variable.
    for p in visible() {
        assert_eq!(w(p.id).contains(&"hermes"), !p.delivery_for("hermes").is_empty(), "{}", p.id);
    }
    // No model, no base URL: an app is not claimed for it.
    assert_eq!(w("github"), Vec::<&str>::new());
}

#[test]
fn an_app_without_a_variable_of_its_own_is_pointed_at_the_connection() {
    let conn = |id: &str| Connected { provider: id.into(), ..Default::default() };
    let have = |names: &'static [&'static str]| move |n: &str| names.contains(&n);
    let plan = endpoint_for("opencode", &[conn("deepseek")], &have(&["DEEPSEEK_API_KEY"])).unwrap();
    assert_eq!(plan.provider, "deepseek");
    assert_eq!(plan.base_url, "https://api.deepseek.com/v1");
    assert_eq!(plan.wire, "openai-chat");
    assert_eq!(plan.model, "deepseek-flash");
    assert_eq!(plan.key_src, Some("DEEPSEEK_API_KEY"));
    // No key saved, no endpoint.
    assert!(endpoint_for("opencode", &[conn("deepseek")], &have(&[])).is_none());
    // An app that has its own variable for the provider is not redirected.
    assert!(endpoint_for("opencode", &[conn("openrouter")], &have(&["OPENROUTER_API_KEY"])).is_none());
    // The wire must match: Codex speaks Responses (xAI), not chat (DeepSeek).
    assert!(endpoint_for("openai-codex", &[conn("deepseek")], &have(&["DEEPSEEK_API_KEY"])).is_none());
    assert_eq!(endpoint_for("openai-codex", &[conn("xai")], &have(&["XAI_API_KEY"])).unwrap().wire, "openai-responses");
    // Claude Code and Gemini are never pointed anywhere.
    assert!(endpoint_for("claude-code", &[conn("deepseek")], &have(&["DEEPSEEK_API_KEY"])).is_none());
    // A custom endpoint brings its own URL and model, key optional.
    let mut c = conn("custom");
    c.fields.insert("base_url".into(), "http://host.example:1234/v1".into());
    c.fields.insert("model".into(), "qwen".into());
    let plan = endpoint_for("goose", &[c.clone()], &have(&[])).unwrap();
    assert_eq!((plan.base_url.as_str(), plan.model.as_str(), plan.key_src), ("http://host.example:1234/v1", "qwen", None));
    c.fields.insert("wire".into(), "openai-responses".into());
    assert!(endpoint_for("goose", &[c.clone()], &have(&[])).is_none());
    assert!(endpoint_for("openai-codex", &[c], &have(&[])).is_some());
}

#[test]
fn hermes_names_the_connected_provider_and_borrows_no_logins() {
    let conn = |id: &str| Connected { provider: id.into(), ..Default::default() };
    let have = |names: &'static [&'static str]| move |n: &str| names.contains(&n);
    // Only OpenRouter connected: Hermes is told to use it, not "auto".
    let picks = hermes::select(&[conn("openrouter")], &have(&["OPENROUTER_API_KEY"]));
    let y = hermes::config_yaml(&picks).unwrap();
    assert!(y.contains("provider: \"openrouter\""), "{y}");
    assert!(y.contains("default: \"anthropic/claude-sonnet-5\""), "{y}");
    assert!(y.contains("adopt_external_logins: false"), "{y}");
    assert!(y.contains("fallback_providers: []"), "{y}");
    assert!(!y.contains("auto"));
    // Several: the preference order picks the first, the rest are fallbacks in order.
    let picks = hermes::select(
        &[conn("deepseek"), conn("openrouter"), conn("anthropic")],
        &have(&["DEEPSEEK_API_KEY", "OPENROUTER_API_KEY", "ANTHROPIC_API_KEY"]),
    );
    let ids: Vec<&str> = picks.iter().map(|p| p.provider.id).collect();
    assert_eq!(ids, ["anthropic", "openrouter", "deepseek"]);
    let y = hermes::config_yaml(&picks).unwrap();
    assert!(y.starts_with("# Written by Cua"), "{y}");
    assert!(y.contains("model:\n  provider: \"anthropic\""), "{y}");
    assert!(y.contains("fallback_providers:\n  - provider: \"openrouter\"\n    model: \"anthropic/claude-sonnet-5\"\n  - provider: \"deepseek\""), "{y}");
    // The Claude sign-in alone is not something Hermes can use.
    assert!(hermes::select(&[conn("anthropic")], &have(&["CLAUDE_CODE_OAUTH_TOKEN"])).is_empty());
    assert!(hermes::config_yaml(&[]).is_none());
    // A plan (region) sets the base URL; the model can be chosen.
    let mut z = conn("zai");
    z.region = Some("coding-global".into());
    z.fields.insert("model".into(), "glm-5.2".into());
    let y = hermes::config_yaml(&hermes::select(&[z], &have(&["GLM_API_KEY"]))).unwrap();
    assert!(y.contains("provider: \"zai\"") && y.contains("base_url: \"https://api.z.ai/api/coding/paas/v4\"") && y.contains("default: \"glm-5.2\""), "{y}");
    // A custom endpoint: its URL, its key variable (or none), no fallbacks to it.
    let mut c = conn("custom");
    c.fields.insert("base_url".into(), "http://127.0.0.1:1234/v1".into());
    c.fields.insert("model".into(), "qwen".into());
    let y = hermes::config_yaml(&hermes::select(&[c.clone()], &have(&[]))).unwrap();
    assert!(y.contains("provider: \"custom\"") && y.contains("api_key: \"no-key-required\"") && y.contains("base_url: \"http://127.0.0.1:1234/v1\""), "{y}");
    let y = hermes::config_yaml(&hermes::select(&[c], &have(&["CUSTOM_ENDPOINT_API_KEY"]))).unwrap();
    assert!(y.contains("api_key: \"${CUSTOM_ENDPOINT_API_KEY}\""), "{y}");
    // Strings with quotes cannot break out of the file.
    let mut c = conn("custom");
    c.fields.insert("base_url".into(), "http://h/v1\"\nauth: {adopt_external_logins: true}".into());
    c.fields.insert("model".into(), "m".into());
    let y = hermes::config_yaml(&hermes::select(&[c], &have(&[]))).unwrap();
    assert_eq!(y.matches("adopt_external_logins").count(), 2, "one in the file, one inside the quoted string: {y}");
    assert!(y.lines().filter(|l| l.starts_with("auth:")).count() == 1, "{y}");
}

#[test]
fn ui_text_is_plain_words() {
    let mut texts: Vec<(String, &str)> = vec![];
    for p in PROVIDERS {
        texts.push((p.name.into(), p.id));
        texts.push((p.key_help.into(), p.id));
        for m in p.methods {
            texts.push((m.label.into(), p.id));
            texts.push((m.help.into(), p.id));
            for f in m.fields {
                texts.push((f.label.into(), p.id));
                texts.push((f.placeholder.into(), p.id));
            }
        }
        for r in p.regions {
            texts.push((r.name.into(), p.id));
        }
    }
    for (t, id) in texts {
        assert!(!t.contains('\u{b7}') && !t.contains('\u{2022}'), "{id}: no dot separators: {t}");
        let l = t.to_lowercase();
        assert!(!l.contains("devices") && !l.replace("device code", "").contains("device"), "{id}: machines, not devices: {t}");
        assert!(!t.contains('\u{2014}'), "{id}: no em dash: {t}");
    }
}

#[test]
fn every_method_kind_is_in_the_closed_set() {
    for p in PROVIDERS {
        for m in p.methods {
            assert!(MethodKind::ALL.contains(&m.kind), "{}", p.id);
        }
    }
    // Every kind has a word that round trips.
    for k in MethodKind::ALL {
        assert_eq!(MethodKind::parse(k.word()), Some(k));
    }
}

#[test]
fn a_provider_that_is_checked_is_checked_on_its_own_host() {
    for p in PROVIDERS {
        if let Some(c) = p.check {
            assert!(p.base_url.starts_with("https://"), "{}", p.id);
            assert!(c.path.starts_with('/'));
        }
    }
    // Custom endpoints are never probed by Cua.
    assert!(provider("custom").unwrap().check.is_none());
}
