// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The prompts the app hands a user to give their AI agent. Each is one line
//! with one link: the line says the job, and the page it links to (the agent
//! docs at `cua.ai/docs/agents/*`, or a one-time `cua.ai/add/<code>` link)
//! says how. Run an agent, onboarding's "Try it" and the First Space guide
//! are not here: there the line is the task itself.
//!
//! The words live here, in the core, so every shell shows the same line, and
//! `docs/scripts/agents-docs.test.ts` checks every link points at a page.

use crate::wizard::SandboxImage;

/// Where the agent pages live.
pub const AGENT_DOCS: &str = "https://cua.ai/docs/agents";

/// "Add a computer": the one-time add link (`https://cua.ai/add/<code>`).
/// `url` is the link the core made, which keeps a non-default relay; without
/// it, the plain link for `code`.
pub fn add_computer_prompt(code: &str, url: Option<&str>) -> String {
    let link = url
        .map(str::to_string)
        .unwrap_or_else(|| format!("https://cua.ai/add/{code}"));
    format!("Add this computer to my Cua Spaces: {link}")
}

/// "Connect a cloud". With a cloud (`AWS`) it names the cloud, and the
/// region and monthly budget in dollars when they are known:
/// `Connect AWS to my Cua Spaces (us-west-2, $200 a month): <link>`.
pub fn connect_cloud_prompt(
    cloud: Option<&str>,
    region: Option<&str>,
    monthly_usd: Option<u32>,
) -> String {
    let link = format!("{AGENT_DOCS}/connect-a-cloud");
    let Some(cloud) = cloud.map(str::trim).filter(|c| !c.is_empty()) else {
        return format!("Connect a cloud to my Cua Spaces: {link}");
    };
    let limits: Vec<String> = region
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_string)
        .into_iter()
        .chain(monthly_usd.map(|usd| format!("${usd} a month")))
        .collect();
    if limits.is_empty() {
        format!("Connect {cloud} to my Cua Spaces: {link}")
    } else {
        format!(
            "Connect {cloud} to my Cua Spaces ({}): {link}",
            limits.join(", ")
        )
    }
}

/// Settings, Storage, "Your S3 bucket": set the bucket up for Cua Volume.
pub fn volume_s3_prompt() -> String {
    format!("Set up S3 storage for my Cua Volume: {AGENT_DOCS}/volume-s3")
}

/// New Space, a cloud that cannot run the image: run it by hand and add it.
pub fn add_machine_prompt(image: &SandboxImage, cloud: &str) -> String {
    format!(
        "Run the {} image on {cloud} and add it to my Cua Spaces: {AGENT_DOCS}/add-a-machine",
        image.name
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_prompt_is_one_line_with_one_link() {
        let prompts = [
            add_computer_prompt("K7QM-4XPB-2RHD", None),
            connect_cloud_prompt(None, None, None),
            connect_cloud_prompt(Some("AWS"), Some("us-west-2"), Some(200)),
            volume_s3_prompt(),
        ];
        for p in prompts {
            assert!(!p.contains('\n'), "{p}");
            assert_eq!(p.matches("https://").count(), 1, "{p}");
            assert!(p.ends_with(|c: char| c.is_ascii_alphanumeric()), "{p}");
            assert!(p.len() < 120, "{p}");
        }
    }

    #[test]
    fn the_words_are_the_approved_ones() {
        assert_eq!(
            add_computer_prompt("K7QM-4XPB-2RHD", None),
            "Add this computer to my Cua Spaces: https://cua.ai/add/K7QM-4XPB-2RHD"
        );
        assert_eq!(
            add_computer_prompt(
                "K7QM-4XPB-2RHD",
                Some("https://cua.ai/add/K7QM-4XPB-2RHD?relay=https%3A%2F%2Fr.example")
            ),
            "Add this computer to my Cua Spaces: https://cua.ai/add/K7QM-4XPB-2RHD?relay=https%3A%2F%2Fr.example"
        );
        assert_eq!(
            connect_cloud_prompt(None, None, None),
            "Connect a cloud to my Cua Spaces: https://cua.ai/docs/agents/connect-a-cloud"
        );
        assert_eq!(
            connect_cloud_prompt(Some("AWS"), Some("us-west-2"), Some(200)),
            "Connect AWS to my Cua Spaces (us-west-2, $200 a month): https://cua.ai/docs/agents/connect-a-cloud"
        );
        assert_eq!(
            connect_cloud_prompt(Some("AWS"), Some(" "), None),
            "Connect AWS to my Cua Spaces: https://cua.ai/docs/agents/connect-a-cloud"
        );
        assert_eq!(
            volume_s3_prompt(),
            "Set up S3 storage for my Cua Volume: https://cua.ai/docs/agents/volume-s3"
        );
    }
}
