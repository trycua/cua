//! `get_guide`: read-only, on-demand sections of the compiled-in skill pack.
//!
//! Most MCP clients hand the model only the server instructions and the tool
//! descriptions. MCP resources such as `skill://cua-driver/SKILL.md` are
//! pull-only and rarely read, while a tool is called. This one returns the
//! guidance for a topic (drag, browser, permissions, ...) from the skill files
//! compiled into this binary, so the text matches the running driver and works
//! with no skill installed.

use std::sync::OnceLock;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::mcp_skills::embedded_file;
use crate::protocol::ToolResult;
use crate::tool::{Tool, ToolDef};
use crate::tool_args::parse_typed_input;

/// The host platform a guide is cut for. Platform guides (`MACOS.md`,
/// `WINDOWS.md`, `LINUX.md`) are all compiled in; a topic reads only the
/// current host's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Macos,
    Windows,
    Linux,
}

impl Platform {
    pub const ALL: [Platform; 3] = [Platform::Macos, Platform::Windows, Platform::Linux];

    pub const fn current() -> Platform {
        if cfg!(target_os = "macos") {
            Platform::Macos
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Platform::Macos => "macOS",
            Platform::Windows => "Windows",
            Platform::Linux => "Linux",
        }
    }
}

/// One heading of one skill file. `only` limits it to a platform.
struct Part {
    file: &'static str,
    heading: &'static str,
    only: Option<Platform>,
}

const fn any(file: &'static str, heading: &'static str) -> Part {
    Part {
        file,
        heading,
        only: None,
    }
}

const fn on(platform: Platform, file: &'static str, heading: &'static str) -> Part {
    Part {
        file,
        heading,
        only: Some(platform),
    }
}

struct Topic {
    name: &'static str,
    summary: &'static str,
    parts: &'static [Part],
}

/// The topics, in the order they are listed. A part names a heading by its
/// exact text; the section runs to the next heading of the same or a higher
/// level. A test resolves every part on every platform, so a renamed heading
/// fails there instead of silently emptying a topic.
const TOPICS: &[Topic] = &[
    Topic {
        name: "overview",
        summary: "the whole SKILL.md: loop, rules, failure map",
        parts: &[any("SKILL.md", "cua-driver")],
    },
    Topic {
        name: "drag",
        summary: "drag and drop: background limits, fallbacks, browser drags",
        parts: &[
            on(
                Platform::Macos,
                "MACOS.md",
                "`delivery_mode` on the pointer family (macOS)",
            ),
            on(
                Platform::Windows,
                "WINDOWS.md",
                "How the contract is enforced per call: the `delivery_mode` field",
            ),
            on(Platform::Windows, "WINDOWS.md", "Intent → tool mapping"),
            on(Platform::Linux, "LINUX.md", "Delivery"),
            any("BROWSER.md", "Extended pointer actions"),
        ],
    },
    Topic {
        name: "browser",
        summary: "page-aware browser tools: bind, snapshot, act, recover",
        parts: &[
            any("BROWSER.md", "Choose the page-aware route first"),
            any("BROWSER.md", "4. Mutate with typed tools"),
            any("BROWSER.md", "Browser chrome and native fallbacks"),
            any("BROWSER.md", "Recovery rules"),
        ],
    },
    Topic {
        name: "permissions",
        summary: "OS permissions and how to get them granted",
        parts: &[
            on(Platform::Macos, "MACOS.md", "Prerequisites: macOS"),
            on(
                Platform::Macos,
                "MACOS.md",
                "Install and approve permissions",
            ),
            on(
                Platform::Windows,
                "WINDOWS.md",
                "Prerequisites: check before starting",
            ),
            on(Platform::Linux, "LINUX.md", "Capture recovery"),
        ],
    },
    Topic {
        name: "batching",
        summary: "run_steps and run_script: several actions in one call",
        parts: &[
            any("WORKFLOW.md", "Batch known actions"),
            any("WORKFLOW.md", "Script a flow"),
        ],
    },
    Topic {
        name: "menus",
        summary: "native menu bars with invoke_menu",
        parts: &[
            on(
                Platform::Macos,
                "MACOS.md",
                "Navigating native menu bars (AXMenuBar)",
            ),
            on(
                Platform::Macos,
                "MACOS.md",
                "Menus opened in the background keep the user's keyboard",
            ),
            on(
                Platform::Windows,
                "WINDOWS.md",
                "Snapshot-bound element mode (preferred)",
            ),
            on(Platform::Linux, "LINUX.md", "Native application menus"),
        ],
    },
];

/// The topic names, in listing order.
pub fn topic_names() -> Vec<&'static str> {
    TOPICS.iter().map(|topic| topic.name).collect()
}

/// A resolved guide: the text to return and where each piece came from.
#[derive(Debug)]
pub struct Guide {
    pub topic: &'static str,
    pub platform: Platform,
    pub text: String,
    /// `(file, heading)` of every section in `text`.
    pub sources: Vec<(&'static str, &'static str)>,
}

/// Why a guide could not be returned.
#[derive(Debug, PartialEq, Eq)]
pub enum GuideError {
    UnknownTopic(String),
    /// A section named by a topic is missing from the embedded files. A test
    /// keeps this from shipping; it is an error rather than a panic so a
    /// broken pack never takes the server down.
    MissingSection {
        file: &'static str,
        heading: &'static str,
    },
}

/// The topic `name` cut for `platform`.
pub fn guide_for(name: &str, platform: Platform) -> Result<Guide, GuideError> {
    let wanted = name.trim().to_ascii_lowercase();
    let topic = TOPICS
        .iter()
        .find(|topic| topic.name == wanted)
        .ok_or_else(|| GuideError::UnknownTopic(name.trim().to_owned()))?;
    let mut text = format!("Cua Driver guide: {} ({})\n", topic.name, platform.label());
    let mut sources = Vec::new();
    let mut files: Vec<&'static str> = Vec::new();
    for part in topic
        .parts
        .iter()
        .filter(|part| part.only.is_none_or(|only| only == platform))
    {
        let section = embedded_file(part.file)
            .and_then(|body| section(body, part.heading))
            .ok_or(GuideError::MissingSection {
                file: part.file,
                heading: part.heading,
            })?;
        text.push_str(&format!("\n=== {} ===\n\n{section}\n", part.file));
        sources.push((part.file, part.heading));
        if !files.contains(&part.file) {
            files.push(part.file);
        }
    }
    let full: Vec<String> = files
        .iter()
        .map(|file| format!("skill://cua-driver/{file}"))
        .collect();
    text.push_str(&format!(
        "\nFull files: {}. Other topics: {}.\n",
        full.join(", "),
        topic_names()
            .into_iter()
            .filter(|other| *other != topic.name)
            .collect::<Vec<_>>()
            .join(", ")
    ));
    Ok(Guide {
        topic: topic.name,
        platform,
        text,
        sources,
    })
}

/// What `get_guide` with no topic returns.
fn index_text() -> String {
    let mut text = String::from("Cua Driver guide topics. Call get_guide({topic}) with one:\n");
    for topic in TOPICS {
        text.push_str(&format!("- {}: {}\n", topic.name, topic.summary));
    }
    text
}

/// A heading line: its level and title, or `None` for any other line.
fn heading_of(line: &str) -> Option<(usize, &str)> {
    let level = line.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let title = line[level..].strip_prefix(' ')?;
    Some((level, title.trim()))
}

/// The section under the heading titled `heading`: the heading line through
/// the line before the next heading of the same or a higher level. Headings
/// inside fenced code blocks (shell comments, for instance) are not headings.
pub fn section<'a>(markdown: &'a str, heading: &str) -> Option<&'a str> {
    let mut fence: Option<char> = None;
    let mut start: Option<(usize, usize)> = None;
    let mut offset = 0;
    for line in markdown.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        let marker = bare.trim_start();
        let opens = ["```", "~~~"]
            .into_iter()
            .find(|fence| marker.starts_with(fence))
            .and_then(|fence| fence.chars().next());
        match (fence, opens) {
            (None, Some(open)) => fence = Some(open),
            (Some(current), Some(close)) if current == close => fence = None,
            _ => {}
        }
        if fence.is_none() && opens.is_none() {
            if let Some((level, title)) = heading_of(bare) {
                match start {
                    None if title == heading => start = Some((offset, level)),
                    Some((from, started)) if level <= started => {
                        return Some(markdown[from..offset].trim_end());
                    }
                    _ => {}
                }
            }
        }
        offset += line.len();
    }
    start.map(|(from, _)| markdown[from..].trim_end())
}

/// One line appended to the description of a tool that most often needs a
/// guide, so an agent that never reads the skill still learns where to look.
pub fn description_hint(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "drag" => "Limits and fallbacks: `get_guide({topic:\"drag\"})`.",
        "get_browser_state" | "browser_prepare" => {
            "Setup and recovery: `get_guide({topic:\"browser\"})`."
        }
        "check_permissions" => "How to get them granted: `get_guide({topic:\"permissions\"})`.",
        "run_steps" | "run_script" => "Batching rules: `get_guide({topic:\"batching\"})`.",
        "invoke_menu" => "Menu paths and refusals: `get_guide({topic:\"menus\"})`.",
        _ => return None,
    })
}

#[derive(Deserialize)]
struct GetGuideInput {
    topic: Option<String>,
}

pub struct GetGuideTool;

static DEF: OnceLock<ToolDef> = OnceLock::new();

fn def() -> &'static ToolDef {
    DEF.get_or_init(|| {
        let topics = topic_names().join(", ");
        ToolDef {
            name: "get_guide".into(),
            description: format!(
                "Read Cua Driver guidance on demand, taken from the guide bundled with this \
                 driver (no skill install needed). Topics: {topics}. Call with no topic to list \
                 them. Read-only."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "description": format!("One of: {topics}. Omit to list the topics.")
                    }
                },
                "additionalProperties": false
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        }
    })
}

#[async_trait]
impl Tool for GetGuideTool {
    fn def(&self) -> &ToolDef {
        def()
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let input = match parse_typed_input::<GetGuideInput>("get_guide", args) {
            Ok(input) => input,
            Err(result) => return result,
        };
        let names = topic_names();
        let Some(topic) = input
            .topic
            .as_deref()
            .map(str::trim)
            .filter(|topic| !topic.is_empty())
        else {
            let topics: Vec<Value> = TOPICS
                .iter()
                .map(|topic| json!({"name": topic.name, "summary": topic.summary}))
                .collect();
            return ToolResult::text(index_text()).with_structured(json!({"topics": topics}));
        };
        match guide_for(topic, Platform::current()) {
            Ok(guide) => {
                let sources: Vec<Value> = guide
                    .sources
                    .iter()
                    .map(|(file, heading)| json!({"file": file, "heading": heading}))
                    .collect();
                ToolResult::text(guide.text).with_structured(json!({
                    "topic": guide.topic,
                    "platform": guide.platform.label(),
                    "sources": sources,
                    "topics": names,
                }))
            }
            Err(GuideError::UnknownTopic(topic)) => ToolResult::error(format!(
                "get_guide: unknown topic {topic:?}. Topics: {}.",
                names.join(", ")
            ))
            .with_structured(json!({
                "code": "unknown_guide_topic",
                "topic": topic,
                "topics": names,
            })),
            Err(GuideError::MissingSection { file, heading }) => ToolResult::error(format!(
                "get_guide: this build's {file} has no section {heading:?}. Read \
                 skill://cua-driver/{file} instead."
            ))
            .with_structured(json!({
                "code": "guide_section_missing",
                "file": file,
                "heading": heading,
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::Content;

    fn text_of(result: &ToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|content| match content {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn run(args: Value) -> ToolResult {
        GetGuideTool.invoke(args).await
    }

    #[test]
    fn every_topic_resolves_on_every_platform() {
        for platform in Platform::ALL {
            for name in topic_names() {
                let guide = guide_for(name, platform)
                    .unwrap_or_else(|error| panic!("{name} on {platform:?}: {error:?}"));
                assert!(
                    !guide.sources.is_empty(),
                    "{name} has no section on {platform:?}"
                );
                assert!(
                    guide.text.len() < 20_000,
                    "{name} on {platform:?} is {} bytes; split the topic instead of flooding the agent's context",
                    guide.text.len()
                );
                for (_, heading) in &guide.sources {
                    assert!(
                        guide.text.contains(heading),
                        "{name}: section {heading:?} missing from the text"
                    );
                }
            }
        }
    }

    #[test]
    fn topics_return_the_guidance_an_agent_needs() {
        let drag = guide_for("drag", Platform::Macos).unwrap().text;
        assert!(drag.contains("drag_and_drop_session"));
        assert!(drag.contains("`browser_pointer` `action:\"drag\"`"));
        let windows_drag = guide_for("drag", Platform::Windows).unwrap().text;
        assert!(windows_drag.contains("Drag / drag-and-drop"));
        assert!(windows_drag.contains("WPF drags"));
        assert!(!windows_drag.contains("drag_and_drop_session"));
        assert!(guide_for("browser", Platform::Linux)
            .unwrap()
            .text
            .contains("browser_prepare"));
        let permissions = guide_for("permissions", Platform::Macos).unwrap().text;
        assert!(permissions.contains("Install and approve permissions"));
        assert!(permissions.contains("Screen Recording"));
        let batching = guide_for("batching", Platform::Windows).unwrap().text;
        assert!(batching.contains("run_steps") && batching.contains("run_script"));
        assert!(guide_for("menus", Platform::Macos)
            .unwrap()
            .text
            .contains("invoke_menu"));
        assert!(guide_for("menus", Platform::Linux)
            .unwrap()
            .text
            .contains("AT-SPI"));
        let overview = guide_for("overview", Platform::Linux).unwrap().text;
        assert!(overview.contains("## Efficient loop"));
        assert!(
            !overview.contains("x-release-please-version"),
            "the overview is the body, not the frontmatter"
        );
    }

    #[test]
    fn a_topic_reads_only_the_hosts_platform_guide() {
        let macos = guide_for("permissions", Platform::Macos).unwrap();
        assert!(macos.sources.iter().all(|(file, _)| *file == "MACOS.md"));
        let linux = guide_for("permissions", Platform::Linux).unwrap();
        assert!(linux.sources.iter().all(|(file, _)| *file == "LINUX.md"));
        assert!(!linux.text.contains("MACOS.md ==="));
    }

    #[test]
    fn topic_names_are_case_and_space_insensitive() {
        assert_eq!(guide_for("  Drag ", Platform::Macos).unwrap().topic, "drag");
    }

    #[tokio::test]
    async fn unknown_topic_is_an_error_that_lists_the_topics() {
        assert_eq!(
            guide_for("nonsense", Platform::Macos).unwrap_err(),
            GuideError::UnknownTopic("nonsense".into())
        );
        let result = run(json!({"topic": "nonsense"})).await;
        assert_eq!(result.is_error, Some(true));
        let message = text_of(&result);
        for name in topic_names() {
            assert!(message.contains(name), "{message}");
        }
        assert!(message.contains("nonsense"));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["code"], "unknown_guide_topic");
        assert_eq!(structured["topics"], json!(topic_names()));
    }

    #[tokio::test]
    async fn no_topic_lists_every_topic() {
        for args in [json!({}), json!({"topic": ""}), json!({"topic": "  "})] {
            let result = run(args).await;
            assert_ne!(result.is_error, Some(true));
            let text = text_of(&result);
            for name in topic_names() {
                assert!(text.contains(&format!("- {name}:")), "{text}");
            }
        }
    }

    #[tokio::test]
    async fn the_tool_returns_a_guide_with_its_sources() {
        let result = run(json!({"topic": "batching"})).await;
        assert_ne!(result.is_error, Some(true));
        let text = text_of(&result);
        assert!(text.starts_with("Cua Driver guide: batching"));
        assert!(text.contains("skill://cua-driver/WORKFLOW.md"));
        let structured = result.structured_content.unwrap();
        assert_eq!(structured["topic"], "batching");
        assert_eq!(structured["sources"][0]["file"], "WORKFLOW.md");
    }

    #[test]
    fn the_tool_is_read_only_and_describes_every_topic() {
        let def = def();
        assert!(def.read_only && !def.destructive && def.idempotent && !def.open_world);
        for name in topic_names() {
            assert!(def.description.contains(name));
        }
    }

    #[test]
    fn section_stops_at_the_next_heading_of_the_same_or_higher_level() {
        let doc = "# T\n\n## A\none\n### A1\ntwo\n## B\nthree\n";
        assert_eq!(section(doc, "A"), Some("## A\none\n### A1\ntwo"));
        assert_eq!(section(doc, "A1"), Some("### A1\ntwo"));
        assert_eq!(section(doc, "B"), Some("## B\nthree"));
        assert_eq!(section(doc, "T"), Some(doc.trim_end()));
        assert_eq!(section(doc, "Missing"), None);
    }

    #[test]
    fn section_ignores_headings_inside_code_fences() {
        let doc = "## A\n```bash\n# not a heading\n## nor this\n```\nafter\n## B\n";
        assert_eq!(
            section(doc, "A"),
            Some("## A\n```bash\n# not a heading\n## nor this\n```\nafter")
        );
        assert_eq!(section(doc, "nor this"), None);
        let tilde = "## A\n~~~\n## x\n~~~\n## B\n";
        assert_eq!(section(tilde, "A"), Some("## A\n~~~\n## x\n~~~"));
    }

    #[test]
    fn hinted_tools_name_a_real_topic() {
        for tool in [
            "drag",
            "get_browser_state",
            "browser_prepare",
            "check_permissions",
            "run_steps",
            "run_script",
            "invoke_menu",
        ] {
            let hint = description_hint(tool).unwrap_or_else(|| panic!("{tool} has no hint"));
            let topic = hint
                .split("topic:\"")
                .nth(1)
                .and_then(|rest| rest.split('"').next())
                .unwrap_or_else(|| panic!("{hint}"));
            assert!(topic_names().contains(&topic), "{tool}: {hint}");
        }
        assert_eq!(description_hint("click"), None);
    }
}
