import { expect, test } from "@playwright/test";

import { mockAuth, mockChatApi } from "./fixtures/mock-api";
import { expectSharedPageShell } from "./fixtures/shell-geometry";

test.describe("shared Chat page shell", () => {
  for (const viewport of [
    { name: "desktop", width: 1440, height: 900 },
    { name: "mobile", width: 390, height: 844 },
  ]) {
    test(`keeps the shared shell and composer in view on ${viewport.name}`, async ({
      page,
    }) => {
      await page.setViewportSize(viewport);
      await mockAuth(page, { admin: false, chat: true });
      await mockChatApi(page);
      await page.goto("/agent");

      await expect(page.getByRole("heading", { name: "Chat" })).toBeVisible();
      await expectSharedPageShell(page);
      await expect(
        page.getByText("Run fleet and browser tasks with the Cua agent."),
      ).toHaveCount(viewport.width > 700 ? 1 : 0);
      await expect(page.locator(".agent-chat-composer")).toBeInViewport();
      const composerBox = await page
        .locator(".agent-chat-composer")
        .boundingBox();
      expect(composerBox).not.toBeNull();
      expect(
        viewport.height - ((composerBox?.y ?? 0) + (composerBox?.height ?? 0)),
      ).toBeLessThan(40);
      expect(
        await page.evaluate(
          () => document.documentElement.scrollHeight - window.innerHeight,
        ),
      ).toBeLessThan(4);

      if (viewport.width > 700) {
        await expect(page.locator(".agent-chat-timestamp").first()).toHaveCSS(
          "color",
          "rgba(164, 173, 187, 0.82)",
        );
        const geometry = await page
          .locator(".cua-pagehead")
          .evaluate(async (element) => {
            const samples: string[] = [];
            for (let frame = 0; frame < 30; frame += 1) {
              await new Promise<void>((resolve) =>
                requestAnimationFrame(() => resolve()),
              );
              const box = element.getBoundingClientRect();
              samples.push(`${box.x}:${box.y}:${box.width}:${box.height}`);
            }
            return samples;
          });
        expect(new Set(geometry).size).toBe(1);
      }
    });
  }
});

test("renders and exercises Chat through the standalone visual preview", async ({
  page,
}) => {
  await page.goto("/agent/preview-browser-task?cua-visual-preview");

  await expect(page).toHaveTitle("Threads · Cua");
  await expect(page.getByRole("heading", { name: "Chat" })).toBeVisible();
  await expect(page.getByText("Threads", { exact: true })).toBeVisible();
  await expect(
    page.getByText("Check the browser fleet and summarize its availability."),
  ).toBeVisible();
  await expect(page.getByText("Command completed")).toBeVisible();
  await expect(
    page.getByText("The browser fleet is healthy:", { exact: false }),
  ).toBeVisible();

  const prompt = page.getByPlaceholder("Ask a question");
  await prompt.fill("Show me the preview response");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(
    page.getByText("Preview mode is connected.", { exact: false }),
  ).toBeVisible();
  await expect(prompt).toBeEnabled();
});

test("sidebar threads replace Chat and mark the route thread current", async ({
  page,
}) => {
  await mockAuth(page, { admin: true, billing: true, chat: true });
  await mockChatApi(page);

  await page.goto("/agent/conversation-1");

  await expect(
    page.getByRole("link", { name: "Chat", exact: true }),
  ).toHaveCount(0);
  const navigation = page.getByRole("navigation", { name: "Main navigation" });
  await expect(navigation.getByText("Threads", { exact: true })).toBeVisible();
  const [poolsBox, usageBox, userApiKeysBox, settingsBox, featureFlagsBox, threadsBox] =
    await Promise.all([
      navigation.getByRole("link", { name: "Pools", exact: true }).boundingBox(),
      navigation.getByRole("link", { name: "Usage", exact: true }).boundingBox(),
      navigation
        .getByRole("link", { name: "API keys", exact: true })
        .boundingBox(),
      navigation
        .getByRole("link", { name: "Settings", exact: true })
        .boundingBox(),
      navigation
        .getByRole("link", { name: "Feature flags", exact: true })
        .boundingBox(),
      navigation.getByText("Threads", { exact: true }).boundingBox(),
    ]);
  expect(poolsBox).not.toBeNull();
  expect(usageBox).not.toBeNull();
  expect(userApiKeysBox).not.toBeNull();
  expect(settingsBox).not.toBeNull();
  expect(featureFlagsBox).not.toBeNull();
  expect(threadsBox).not.toBeNull();
  expect(poolsBox!.y + poolsBox!.height).toBeLessThanOrEqual(usageBox!.y);
  expect(usageBox!.y + usageBox!.height).toBeLessThanOrEqual(userApiKeysBox!.y);
  expect(userApiKeysBox!.y + userApiKeysBox!.height).toBeLessThanOrEqual(
    settingsBox!.y,
  );
  expect(settingsBox!.y + settingsBox!.height).toBeLessThanOrEqual(
    featureFlagsBox!.y,
  );
  expect(featureFlagsBox!.y + featureFlagsBox!.height).toBeLessThanOrEqual(
    threadsBox!.y,
  );
  await expect(
    page.getByRole("link", { name: "Example browser task" }),
  ).toHaveAttribute("aria-current", "page");
  await expect(
    page.getByRole("link", { name: "Example browser task" }),
  ).toHaveAttribute("title", "Example browser task");

  await page.goto("/agent/archived");
  await expect(
    page.getByRole("link", { name: "Archived threads" }),
  ).toHaveAttribute("aria-current", "page");
});

test("new thread posts before navigating", async ({ page }) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    conversations: [
      {
        id: "conversation-1",
        title: "Existing thread",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/conversation-1");
  await page.getByRole("link", { name: "New thread" }).click();

  await expect.poll(() => chat.createRequests).toBe(1);
  await expect(page).toHaveURL(/\/agent\/conversation-2$/);
});

test("newest active thread redirects from /agent", async ({ page }) => {
  const now = Date.now();
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [
      {
        id: "older-thread",
        title: "Older thread",
        created_at: new Date(now - 120_000).toISOString(),
        updated_at: new Date(now - 120_000).toISOString(),
        messages: [],
      },
      {
        id: "newest-thread",
        title: "Newest thread",
        created_at: new Date(now - 60_000).toISOString(),
        updated_at: new Date(now - 60_000).toISOString(),
        messages: [],
      },
    ],
  });

  await page.goto("/agent");

  await expect(page).toHaveURL(/\/agent\/newest-thread$/);
});

test("newest active creates a thread when /agent has no active threads", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, { conversations: [] });

  await page.goto("/agent");

  await expect.poll(() => chat.createRequests).toBe(1);
  await expect(page).toHaveURL(/\/agent\/conversation-1$/);
});

test("native New thread follow ignores a live pending create", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    holdCreate: true,
    conversations: [
      {
        id: "conversation-1",
        title: "Existing thread",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/conversation-1");
  await page.getByRole("link", { name: "New thread" }).click();
  await expect.poll(() => chat.createRequests).toBe(1);
  await page.getByRole("link", { name: "New thread" }).click();
  await expect.poll(() => chat.createRequests).toBe(1);

  chat.releaseCreate();
  await expect(page).toHaveURL(/\/agent\/conversation-2$/);
});

test("stale active-list responses cannot erase a newly created thread", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    holdList: true,
    conversations: [
      {
        id: "conversation-1",
        title: "Existing thread",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/conversation-1");
  await expect.poll(() => chat.listRequests).toBeGreaterThan(0);
  await page.getByRole("link", { name: "New thread" }).click();
  await expect(page).toHaveURL(/\/agent\/conversation-2$/);

  chat.releaseList();
  await expect(
    page.getByRole("link", { name: "New conversation" }),
  ).toBeVisible();
  await expect(page.getByRole("link", { name: "New thread" })).toBeVisible();
});

test("chat flag gating does not request active threads", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: false });
  const chat = await mockChatApi(page);

  const config = page.waitForResponse("**/api/config");
  await page.goto("/agent");
  await config;

  await expect(page).toHaveURL(/\/pools$/);
  await expect.poll(() => chat.listRequests).toBe(0);
});

test("thread list failure shows retry without creating a thread", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    conversations: [],
    listError: { message: "Thread list unavailable" },
  });

  await page.goto("/agent");

  await expect(page.getByText("Unable to load threads")).toBeVisible();
  await expect(page.getByRole("button", { name: "Retry" })).toBeVisible();
  await expect.poll(() => chat.createRequests).toBe(0);
});

test("thread actions are contextual and do not navigate before selection", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [
      {
        id: "conversation-a",
        title: "Context thread",
        created_at: now,
        updated_at: now,
        messages: [],
      },
      {
        id: "conversation-b",
        title: "Other thread",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/conversation-a");
  await page
    .getByRole("button", { name: "Actions for Context thread" })
    .click();

  await expect(page).toHaveURL(/\/agent\/conversation-a$/);
  await expect(page.getByRole("menuitem", { name: "Archive" })).toBeVisible();
});

test("archiving the active thread falls back and Undo restores it", async ({
  page,
}) => {
  const now = Date.now();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    conversations: [
      {
        id: "conversation-a",
        title: "Newest thread",
        created_at: new Date(now - 60_000).toISOString(),
        updated_at: new Date(now - 60_000).toISOString(),
        messages: [],
      },
      {
        id: "conversation-b",
        title: "Older thread",
        created_at: new Date(now - 120_000).toISOString(),
        updated_at: new Date(now - 120_000).toISOString(),
        messages: [],
      },
    ],
  });

  await page.goto("/agent/conversation-a");
  await page.getByRole("button", { name: "Actions for Newest thread" }).click();
  await page.getByRole("menuitem", { name: "Archive" }).click();

  await expect
    .poll(() => chat.patchRequests)
    .toEqual([{ conversationId: "conversation-a", archived: true }]);
  await expect(page).toHaveURL(/\/agent\/conversation-b$/);
  await page.getByRole("button", { name: "Undo" }).click();
  await expect
    .poll(() => chat.patchRequests)
    .toEqual([
      { conversationId: "conversation-a", archived: true },
      { conversationId: "conversation-a", archived: false },
    ]);
  await expect(page.getByRole("link", { name: "Newest thread" })).toBeVisible();
});

test("delayed archive completion preserves navigation away from the archived thread", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    holdPatch: true,
    conversations: [
      {
        id: "conversation-a",
        title: "Archive race",
        created_at: now,
        updated_at: now,
        messages: [],
      },
      {
        id: "conversation-b",
        title: "Other thread",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/conversation-a");
  await page.getByRole("button", { name: "Actions for Archive race" }).click();
  await page.getByRole("menuitem", { name: "Archive" }).click();
  await expect
    .poll(() => chat.patchRequests)
    .toEqual([{ conversationId: "conversation-a", archived: true }]);

  await page.getByRole("link", { name: "Pools", exact: true }).click();
  await expect(page).toHaveURL(/\/pools$/);
  chat.releasePatch();

  await expect(page).toHaveURL(/\/pools$/);
});

test("mobile thread navigation closes the AppLayout drawer after selection", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await page.setViewportSize({ width: 390, height: 844 });
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [
      {
        id: "conversation-a",
        title: "Mobile first",
        created_at: now,
        updated_at: now,
        messages: [],
      },
      {
        id: "conversation-b",
        title: "Mobile second",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/conversation-a");
  await expect(
    page.getByRole("button", { name: "View conversations" }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Open navigation" }).click();
  await page.getByRole("link", { name: "Mobile second" }).click();

  await expect(page).toHaveURL(/\/agent\/conversation-b$/);
  await expect(
    page.getByRole("button", { name: "Close navigation" }),
  ).toHaveCount(0);
});

test("offers a useful first-run Chat state in the standalone preview", async ({
  page,
}) => {
  await page.goto("/agent?cua-visual-preview&cua-preview-state=empty");

  await expect(page.getByRole("heading", { name: "Chat" })).toBeVisible();
  await expect(
    page.getByText(
      "Choose a thread from navigation or create a new thread to begin.",
    ),
  ).toBeVisible();
  await expect(page.getByPlaceholder("Ask a question")).toBeEnabled();
});

test("renders full-width routed chat without local conversation controls", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page);

  await page.goto("/agent/conversation-1");

  await expect(page.locator(".agent-chat-history")).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "View conversations" }),
  ).toHaveCount(0);
  await expect(page.getByRole("region", { name: "Chat" })).toBeVisible();
  await expect(page.getByPlaceholder("Ask a question")).toBeVisible();
  const mainBox = await page.locator(".agent-chat-main").boundingBox();
  const pageBodyBox = await page.locator(".cua-pagebody").boundingBox();
  expect(mainBox).not.toBeNull();
  expect(pageBodyBox).not.toBeNull();
  expect(
    Math.abs((mainBox?.x ?? 0) - (pageBodyBox?.x ?? 0)),
  ).toBeLessThanOrEqual(32);
  expect(
    Math.abs(
      (mainBox?.x ?? 0) +
        (mainBox?.width ?? 0) -
        ((pageBodyBox?.x ?? 0) + (pageBodyBox?.width ?? 0)),
    ),
  ).toBeLessThanOrEqual(32);
  expect(chat.authorizationHeaders.length).toBeGreaterThan(0);
  expect(chat.authorizationHeaders).toEqual(
    expect.arrayContaining([expect.stringMatching(/^Bearer /)]),
  );
});

test("renders a full-width multiline composer with hidden announcements", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page);

  await page.goto("/agent");

  const composer = page.locator(".agent-chat-composer");
  const prompt = page.locator(".agent-chat-prompt");
  const textarea = page.getByPlaceholder("Ask a question");
  const composerBox = await composer.boundingBox();
  const promptBox = await prompt.boundingBox();
  const initialHeight = (await textarea.boundingBox())?.height ?? 0;

  expect(composerBox).not.toBeNull();
  expect(promptBox).not.toBeNull();
  expect((promptBox?.width ?? 0) / (composerBox?.width ?? 1)).toBeGreaterThan(
    0.9,
  );
  expect(initialHeight).toBeGreaterThan(44);

  await textarea.fill("First line\nSecond line\nThird line");
  await expect
    .poll(async () => (await textarea.boundingBox())?.height ?? 0)
    .toBeGreaterThan(initialHeight);

  const announcement = page
    .locator('[aria-live="polite"]')
    .filter({ hasText: "Conversation loaded" });
  const announcements = page.locator(".agent-chat-announcements");
  await expect(announcement).toBeAttached();
  await expect(announcements).toHaveCSS("overflow", "hidden");
  await expect
    .poll(async () =>
      announcements.evaluate((element) => ({
        width: element.clientWidth,
        height: element.clientHeight,
      })),
    )
    .toEqual({ width: 1, height: 1 });
});

test("keeps the prompt visible while the routed conversation loads", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, { holdConversation: true });

  await page.goto("/agent/conversation-1");

  await expect(page.getByTestId("message-skeleton")).toHaveCount(2);
  await expect(page.getByPlaceholder("Ask a question")).toBeVisible();
  chat.releaseConversation();
});

test("loads a selected conversation with message skeletons and accessible author times", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, { holdConversation: true });

  await page.goto("/agent/conversation-1");

  await expect(page.getByTestId("message-skeleton")).toHaveCount(2);
  await expect(page.getByPlaceholder("Ask a question")).toBeVisible();
  chat.releaseConversation();

  await expect(page.getByText("Open the example site.")).toBeVisible();
  await expect(page.getByText("The example site is ready.")).toBeVisible();
  await expect(page.getByLabel("Your avatar")).toHaveText("Y");
  await expect(page.getByLabel(/^You at /)).toBeVisible();
  await expect(page.getByLabel(/^Assistant at /)).toBeVisible();
});

test("renders and sanitizes Markdown in user and assistant messages", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [
      {
        id: "markdown-chat",
        title: "Markdown chat",
        created_at: now,
        updated_at: now,
        messages: [
          {
            id: "markdown-user",
            role: "user",
            content: [
              "| Request | Value |",
              "| --- | --- |",
              "| Format | **Markdown** |",
              "",
              "[Safe link](https://example.com) [Unsafe link](javascript:alert(1))",
              "",
              '<script>window.__markdownXss = true</script><img src="https://example.com/tracker.png"><span id="location" style="color:red" onclick="window.__markdownXss = true">Sanitized text</span>',
            ].join("\n"),
            created_at: now,
          },
          {
            id: "markdown-assistant",
            role: "assistant",
            content: [
              "| Pool | Replicas | Available | Status |",
              "| --- | ---: | ---: | --- |",
              "| [demo](/pools/team/demo) | 2 | 1 | Ready |",
              "",
              "```ts",
              "const ready = true",
              "```",
            ].join("\n"),
            created_at: now,
          },
        ],
      },
    ],
  });

  await page.goto("/agent/markdown-chat");

  const userBubble = page.getByLabel(/^You at /);
  const assistantBubble = page.getByLabel(/^Assistant at /);
  await expect(userBubble.getByRole("table")).toBeVisible();
  await expect(
    userBubble.getByRole("link", { name: "Safe link" }),
  ).toHaveAttribute("href", "https://example.com");
  await expect(userBubble.getByRole("link", { name: "Safe link" })).toHaveCSS(
    "color",
    "rgb(159, 215, 255)",
  );
  await expect(userBubble.getByText("Unsafe link")).not.toHaveAttribute(
    "href",
    /.+/,
  );
  await expect(assistantBubble.getByRole("table")).toBeVisible();
  await expect(assistantBubble.getByRole("columnheader", { name: "Namespace" })).toHaveCount(0);
  await expect(assistantBubble.getByRole("link", { name: "demo" })).toHaveAttribute(
    "href",
    "/pools/team/demo",
  );
  await expect(assistantBubble.locator("pre code")).toContainText(
    "const ready = true",
  );

  await expect(userBubble.locator("script, img")).toHaveCount(0);
  const sanitizedSpan = userBubble.getByText("Sanitized text");
  await expect(sanitizedSpan).not.toHaveAttribute("style");
  await expect(sanitizedSpan).not.toHaveAttribute("onclick");
  await expect(sanitizedSpan).not.toHaveAttribute("id");
  expect(
    await page.evaluate(
      () => (window as Window & { __markdownXss?: boolean }).__markdownXss,
    ),
  ).toBeUndefined();
});

test("does not redirect while chat feature configuration is pending", async ({
  page,
}) => {
  const auth = await mockAuth(
    page,
    { admin: false, chat: true },
    { holdConfig: true },
  );
  await mockChatApi(page);

  await page.goto("/agent");
  await expect(page).toHaveURL(/\/agent$/);
  await expect(page.getByRole("heading", { name: "Chat" })).toHaveCount(1);

  auth.releaseConfig();
  await expect(page).toHaveURL(/\/agent\/conversation-1$/);
});

test("opens archived thread titles through routed navigation", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [
      {
        id: "archived-open",
        title: "Open archived",
        created_at: now,
        updated_at: now,
        archived_at: now,
        messages: [
          {
            id: "archived-message",
            role: "assistant",
            content: "Archived route transcript",
            created_at: now,
          },
        ],
      },
    ],
  });
  await page.goto("/agent/archived");
  await expect(page.getByRole("link", { name: "Open archived" })).toHaveAttribute(
    "title",
    "Open archived",
  );
  await page.getByRole("link", { name: "Open archived" }).click();
  await expect(page).toHaveURL(/\/agent\/archived-open$/);
  await expect(page.getByText("Archived route transcript")).toBeVisible();
});

test("suppresses duplicate transcript restore and does not hijack later navigation", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    holdPatch: true,
    conversations: [
      {
        id: "archived-race",
        title: "Archived race",
        created_at: now,
        updated_at: now,
        archived_at: now,
        messages: [],
      },
    ],
  });
  await page.goto("/agent/archived-race");
  const restore = page.getByRole("button", { name: "Restore thread" });
  await restore.click();
  await restore.click({ force: true });
  await expect
    .poll(() => chat.patchRequests)
    .toEqual([{ conversationId: "archived-race", archived: false }]);
  await page.goto("/pools");
  chat.releasePatch();
  await expect(page).toHaveURL(/\/pools$/);
});

test("does not overwrite an active thread when a delayed archived restore finishes", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    holdPatch: true,
    conversations: [
      {
        id: "archived-a",
        title: "Archived A",
        created_at: now,
        updated_at: now,
        archived_at: now,
        messages: [
          {
            id: "archived-a-message",
            role: "assistant",
            content: "Archived A transcript",
            created_at: now,
          },
        ],
      },
      {
        id: "active-b",
        title: "Active B",
        created_at: now,
        updated_at: now,
        messages: [
          {
            id: "active-b-message",
            role: "assistant",
            content: "Active B transcript",
            created_at: now,
          },
        ],
      },
    ],
  });

  await page.goto("/agent/archived-a");
  await page.getByRole("button", { name: "Restore thread" }).click();
  await expect
    .poll(() => chat.patchRequests)
    .toEqual([{ conversationId: "archived-a", archived: false }]);
  await page.getByRole("link", { name: "Active B" }).click();
  await expect(page).toHaveURL(/\/agent\/active-b$/);
  await expect(page.getByText("Active B transcript")).toBeVisible();
  const listRequestsBeforeRelease = chat.listRequests;
  chat.releasePatch();
  await expect.poll(() => chat.listRequests).toBeGreaterThan(
    listRequestsBeforeRelease,
  );
  await expect(page).toHaveURL(/\/agent\/active-b$/);
  await expect(page.getByText("Active B transcript")).toBeVisible();
  await expect(page.getByText("Archived A transcript")).toHaveCount(0);
});

test("blocks sending while an archived routed conversation loads", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    holdConversation: true,
    conversations: [
      {
        id: "archived-loading",
        title: "Archived loading",
        created_at: now,
        updated_at: now,
        archived_at: now,
        messages: [],
      },
    ],
  });
  await page.goto("/agent/archived-loading");
  await expect(page.getByPlaceholder("Ask a question")).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Send message" }),
  ).toBeDisabled();
  chat.releaseConversation();
  await expect(page.getByPlaceholder("Ask a question")).toHaveCount(0);
});

test("turn 409 becomes archived read-only state without retry", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [
      {
        id: "turn-409",
        title: "Turn 409",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
    turns: [
      {
        archiveBeforeError: true,
        error: { status: 409, message: "conversation is archived" },
      },
    ],
  });
  await page.goto("/agent/turn-409");
  await page.getByPlaceholder("Ask a question").fill("Race archive");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(page.getByText(/archived while generating/)).toBeVisible();
  await expect(page.getByPlaceholder("Ask a question")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Retry" })).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Restore thread" }),
  ).toBeVisible();
});

test("turn 409 remains read-only when reconciliation fails", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [
      {
        id: "turn-409-refresh-failure",
        title: "Turn 409 refresh failure",
        created_at: now,
        updated_at: now,
        messages: [
          {
            id: "persisted-message",
            role: "assistant",
            content: "Persisted transcript",
            created_at: now,
          },
        ],
      },
    ],
    refreshError: { afterTurnRequests: 1, message: "Refresh unavailable" },
    turns: [
      {
        archiveBeforeError: true,
        error: { status: 409, message: "conversation is archived" },
      },
    ],
  });
  await page.goto("/agent/turn-409-refresh-failure");
  await page.getByPlaceholder("Ask a question").fill("Race archive");
  await page.getByRole("button", { name: "Send message" }).click();

  await expect(page.getByText(/archived while generating/)).toBeVisible();
  await expect(page.getByText("Persisted transcript")).toBeVisible();
  await expect(page.getByPlaceholder("Ask a question")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Retry" })).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Restore thread" }),
  ).toBeVisible();
  await expect(page.getByRole("alert")).toContainText("Refresh unavailable");
  await expect(
    page.getByRole("button", { name: "Refresh conversation" }),
  ).toBeVisible();
});

test("keeps chat contained in a short desktop viewport", async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 500 });
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page);
  await page.goto("/agent/conversation-1");
  await expect(page.getByPlaceholder("Ask a question")).toBeInViewport();
  await expect
    .poll(() =>
      page.evaluate(
        () => document.documentElement.scrollHeight - window.innerHeight,
      ),
    )
    .toBeLessThan(4);
});

test("recovers from New thread creation failure without a stuck route", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    createError: { message: "Create failed" },
    conversations: [
      {
        id: "usable-thread",
        title: "Usable thread",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
  });
  await page.goto("/agent/usable-thread");
  await page.getByRole("link", { name: "New thread" }).click();
  await expect(page).toHaveURL(/\/agent\/usable-thread$/);
  await expect(page.getByPlaceholder("Ask a question")).toBeEnabled();
  await page.getByRole("link", { name: "New thread" }).click();
  await expect.poll(() => chat.createRequests).toBe(2);
  await expect(page).toHaveURL(/\/agent\/conversation-2$/);
});

test("lists archived threads and restores them from the archive view", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    conversations: [
      {
        id: "archived-thread",
        title: "Archived transcript",
        created_at: now,
        updated_at: now,
        archived_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/archived");

  await expect(
    page.getByRole("heading", { name: "Archived threads" }),
  ).toBeVisible();
  await expect(
    page.getByRole("link", { name: "Archived transcript" }),
  ).toHaveAttribute("href", "#/agent/archived-thread");
  await page
    .getByRole("button", { name: "Restore Archived transcript" })
    .click();
  await expect
    .poll(() => chat.patchRequests)
    .toEqual([{ conversationId: "archived-thread", archived: false }]);
  await expect(page.getByText("No archived threads.")).toBeVisible();
});

test("renders archived threads read-only and restores from the transcript", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    conversations: [
      {
        id: "archived-thread",
        title: "Archived transcript",
        created_at: now,
        updated_at: now,
        archived_at: now,
        messages: [
          {
            id: "archived-message",
            role: "assistant",
            content: "Read-only history",
            created_at: now,
          },
        ],
      },
    ],
  });

  await page.goto("/agent/archived-thread");

  await expect(
    page
      .getByRole("region", { name: "Chat" })
      .getByText("This thread is archived and read-only.", { exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Read-only history")).toBeVisible();
  await expect(page.getByPlaceholder("Ask a question")).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Stop generating" }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Restore thread" }).click();
  await expect
    .poll(() => chat.patchRequests)
    .toEqual([{ conversationId: "archived-thread", archived: false }]);
  await expect(page).toHaveURL(/\/agent\/archived-thread$/);
  await expect(page.getByPlaceholder("Ask a question")).toBeVisible();
});

test("successful restore survives an active-list refresh failure", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    listErrorAfterPatch: { message: "Active list unavailable" },
    conversations: [
      {
        id: "archived-conversation",
        title: "Restore despite refresh",
        created_at: now,
        updated_at: now,
        archived_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/archived-conversation");
  await page.getByRole("button", { name: "Restore thread" }).click();

  await expect.poll(() => chat.patchRequests.length).toBe(1);
  await expect(page.getByText("This thread is archived and read-only.")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Send message" })).toBeVisible();
  await expect(page.getByText("Unable to restore thread")).toHaveCount(0);
});

test("archived restore is disabled during another thread mutation", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    holdCreate: true,
    conversations: [
      {
        id: "archived-conversation",
        title: "Archived mutation target",
        created_at: now,
        updated_at: now,
        archived_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/archived");
  await page.getByRole("link", { name: "New thread" }).click();
  await expect.poll(() => chat.createRequests).toBe(1);
  await expect(
    page.getByRole("button", { name: "Restore Archived mutation target" }),
  ).toBeDisabled();

  chat.releaseCreate();
});

test("returns focus to the prompt after keyboard submission completes", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [],
    turns: [
      {
        events: [
          { type: "content_delta", delta: "Keyboard complete." },
          assistant("Keyboard complete."),
        ],
      },
    ],
  });

  await page.goto("/agent");
  const prompt = page.getByPlaceholder("Ask a question");
  await prompt.fill("Submit from keyboard");
  await prompt.press("Enter");

  await expect(
    page.getByText("Keyboard complete.", { exact: true }),
  ).toBeVisible();
  await expect(
    page
      .locator('[aria-live="polite"]')
      .filter({ hasText: /^Latest assistant message available\.$/ }),
  ).toHaveCount(1);
  await expect(
    page
      .locator("[aria-live]")
      .filter({ hasText: /Latest user message available/ }),
  ).toHaveCount(0);
  await expect(prompt).toBeEnabled();
  await expect(prompt).toBeFocused();
});

test("exposes chat states and controls by accessible role and unique name", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    conversations: [
      {
        id: "accessible-chat",
        title: "Accessible chat",
        created_at: now,
        updated_at: now,
        messages: [
          {
            id: "user-one",
            role: "user",
            content: "First question",
            created_at: now,
          },
          {
            id: "user-two",
            role: "user",
            content: "Second question",
            created_at: now,
          },
        ],
      },
    ],
    turns: [
      {
        hold: true,
        events: [
          assistant("", [bashCall("accessible-command", "printf accessible")]),
        ],
      },
      { error: { status: 500, message: "Accessible failure" } },
    ],
  });

  await page.goto("/agent/accessible-chat");
  const transcript = page.getByRole("region", { name: "Chat" });
  await expect(transcript).toBeVisible();
  const bubbleLabels = await transcript
    .locator("[aria-label]")
    .evaluateAll((elements) =>
      elements
        .map((element) => element.getAttribute("aria-label"))
        .filter((label) => label?.startsWith("You at ")),
    );
  expect(new Set(bubbleLabels).size).toBe(bubbleLabels.length);

  const prompt = page.getByPlaceholder("Ask a question");
  await prompt.fill("Run accessible command");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(
    page
      .locator('[aria-live="polite"]')
      .filter({ hasText: "Generating response" }),
  ).toHaveCount(1);
  await expect(transcript.getByText("Thinking...", { exact: true })).toBeVisible();
  await expect(
    transcript.locator('[role="status"], [aria-live]'),
  ).toHaveCount(0);
  await expect(
    page.locator('.agent-chat-announcements[aria-live="polite"]'),
  ).toHaveCount(1);
  await expect(
    page.getByRole("button", { name: "Stop generating" }),
  ).toBeVisible();
  chat.releaseTurn();
  await expect(
    transcript.getByText("Thinking...", { exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Command details" }),
  ).toBeVisible();
  await expect(page.getByRole("alert")).toContainText("Accessible failure");
  await expect(
    page.locator("[aria-live]").filter({ hasText: "Accessible failure" }),
  ).toHaveCount(0);
});

test("shows immediate pending feedback until the first assistant delta", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    conversations: [],
    turns: [
      {
        hold: true,
        events: [
          { type: "content_delta", delta: "Ready." },
          assistant("Ready."),
        ],
      },
    ],
  });

  await page.goto("/agent");
  const transcript = page.getByRole("region", { name: "Chat" });
  await page.getByPlaceholder("Ask a question").fill("Wait for a response");
  await page.getByRole("button", { name: "Send message" }).click();

  await expect(
    transcript.getByText("Wait for a response", { exact: true }),
  ).toBeVisible();
  await expect(transcript.getByText("Thinking...", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Stop generating" }),
  ).toBeVisible();
  await expect(
    transcript.locator('[role="status"], [aria-live]'),
  ).toHaveCount(0);
  await expect(
    page.locator('.agent-chat-announcements[aria-live="polite"]'),
  ).toHaveCount(1);

  chat.releaseTurn();
  await expect(
    transcript.getByText("Thinking...", { exact: true }),
  ).toHaveCount(0);
  await expect(transcript.getByText("Ready.", { exact: true })).toBeVisible();
});

test("announces generating response for a held second turn", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [],
    turns: [
      {
        events: [
          { type: "content_delta", delta: "First response." },
          assistant("First response."),
        ],
      },
      { hold: true },
    ],
  });

  await page.goto("/agent");
  const announcements = page.locator(".agent-chat-announcements");
  await page.getByPlaceholder("Ask a question").fill("First turn");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(page.getByText("First response.", { exact: true })).toBeVisible();
  await expect(announcements).toHaveText("Latest assistant message available.");

  await page.getByPlaceholder("Ask a question").fill("Second turn");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(page.getByText("Thinking...", { exact: true })).toBeVisible();
  await expect(announcements).toHaveText("Generating response");
  await page.getByRole("button", { name: "Stop generating" }).click();
});

test("scrolls the newest message into view while keeping the composer fixed", async ({
  page,
}) => {
  const now = new Date().toISOString();
  const messages = Array.from({ length: 48 }, (_, index) => ({
    id: `overflow-${index}`,
    role: index % 2 === 0 ? ("user" as const) : ("assistant" as const),
    content: `Overflow message ${index + 1} with enough text to occupy transcript space.`,
    created_at: now,
  }));
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [
      {
        id: "overflow-chat",
        title: "Overflow chat",
        created_at: now,
        updated_at: now,
        messages,
      },
    ],
    turns: [
      {
        events: [
          { type: "content_delta", delta: "Newest response." },
          assistant("Newest response."),
        ],
      },
    ],
  });

  await page.goto("/agent/overflow-chat");
  const transcript = page.getByRole("region", { name: "Chat" });
  const composer = page.locator(".agent-chat-composer");
  const composerBefore = await composer.boundingBox();
  await transcript.evaluate((element) => {
    element.scrollTop = 0;
  });

  await page
    .getByPlaceholder("Ask a question")
    .fill("Show the newest response");
  await page.getByRole("button", { name: "Send message" }).click();

  await expect(
    page.getByText("Newest response.", { exact: true }),
  ).toBeInViewport();
  await expect
    .poll(() => transcript.evaluate((element) => element.scrollTop))
    .toBeGreaterThan(0);
  const composerAfter = await composer.boundingBox();
  expect(composerBefore).not.toBeNull();
  expect(composerAfter).not.toBeNull();
  expect(composerAfter?.y).toBeCloseTo(composerBefore?.y ?? 0, 0);
});

test("contains long chat history without inflating document scroll", async ({
  page,
}) => {
  await page.setViewportSize({ width: 1920, height: 1080 });
  const now = new Date().toISOString();
  const messages = Array.from({ length: 48 }, (_, index) => ({
    id: `windows-history-${index}`,
    role: index % 2 === 0 ? ("user" as const) : ("assistant" as const),
    content: `| Column | Value |\n| --- | --- |\n| Row ${index} | ${"windows-image-value ".repeat(8)} |`,
    created_at: now,
  }));
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [
      {
        id: "windows-history",
        title: "Windows history",
        created_at: now,
        updated_at: now,
        messages,
      },
    ],
  });

  await page.goto("/agent/windows-history");

  const transcript = page.getByRole("region", { name: "Chat" });
  await expect
    .poll(() =>
      transcript.evaluate(
        (element) => element.scrollHeight - element.clientHeight,
      ),
    )
    .toBeGreaterThan(1000);
  await expect
    .poll(() =>
      page.evaluate(
        () => document.documentElement.scrollHeight - window.innerHeight,
      ),
    )
    .toBeLessThan(100);
});

test("parses fragmented unterminated NDJSON and rejects malformed final events", async ({
  page,
}) => {
  await mockAuth(page);
  await page.goto("/pools");

  const result = await page.evaluate(async () => {
    const api = await import("/src/api/chat.ts");
    const originalFetch = window.fetch;
    const encode = (value: string) => new TextEncoder().encode(value);
    const stream = (chunks: string[]) =>
      new ReadableStream<Uint8Array>({
        start(controller) {
          for (const chunk of chunks) controller.enqueue(encode(chunk));
          controller.close();
        },
      });

    try {
      window.fetch = async () =>
        new Response(
          stream([
            '{"type":"content_delta","delta":"Hel',
            'lo"}\n{"type":"assistant","message":{"role":"assistant","content":"Complete"}}',
          ]),
          { status: 200 },
        );
      const deltas: string[] = [];
      const complete = await api.streamTurn("conversation-1", [], (delta) =>
        deltas.push(delta),
      );

      window.fetch = async () =>
        new Response(
          stream([
            '{"type":"assistant","message":{"role":"assistant","content":"ok","tool_calls":{}}}',
          ]),
          { status: 200 },
        );
      let malformedError = "";
      try {
        await api.streamTurn("conversation-1", [], () => undefined);
      } catch (error) {
        malformedError = error instanceof Error ? error.name : String(error);
      }

      return { complete, deltas, malformedError };
    } finally {
      window.fetch = originalFetch;
    }
  });

  expect(result.deltas).toEqual(["Hello"]);
  expect(result.complete.content).toBe("Complete");
  expect(result.malformedError).toBe("ChatApiError");
});

test("hides chat navigation and redirects when the feature is disabled", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: false });

  await page.goto("/pools");
  await expect(page.getByRole("link", { name: "Chat" })).toHaveCount(0);

  await page.goto("/agent");
  await expect(page).toHaveURL(/\/pools$/);
});

test("disables skeleton shimmer when reduced motion is requested", async ({
  page,
}) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, { holdConversation: true });

  await page.goto("/agent/conversation-1");

  await expect(page.getByTestId("message-skeleton").first()).toBeVisible();
  await expect(page.getByTestId("message-skeleton").first()).toHaveCSS(
    "animation-name",
    "none",
  );
});

test("disables thinking-dot animation under reduced motion", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" });
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, { conversations: [], turns: [{ hold: true }] });

  await page.goto("/agent");
  await page.getByPlaceholder("Ask a question").fill("Wait without motion");
  await page.getByRole("button", { name: "Send message" }).click();

  const dots = page.locator(".agent-chat-thinking-dots > span");
  await expect(dots).toHaveCount(3);
  for (const dot of await dots.all()) {
    await expect(dot).toBeVisible();
    await expect(dot).toHaveCSS("animation-name", "none");
  }
  await page.getByRole("button", { name: "Stop generating" }).click();
});

const bashCall = (
  id: string,
  command: string,
  options: Record<string, number> = {},
) => ({
  id,
  type: "function" as const,
  function: {
    name: "bash",
    arguments: JSON.stringify({ command, ...options }),
  },
});

const assistant = (
  content: string,
  toolCalls: ReturnType<typeof bashCall>[] = [],
) => ({
  type: "assistant",
  message: { role: "assistant", content, tool_calls: toolCalls },
});

test("runs a browser bash tool loop and shows completed command details", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [],
    turns: [
      {
        events: [
          assistant("", [
            bashCall("bash-1", "printf hello > note.txt; cat note.txt"),
          ]),
        ],
      },
      {
        events: [
          { type: "content_delta", delta: "The file contains hello." },
          assistant("The file contains hello."),
        ],
      },
    ],
  });

  await page.goto("/agent");
  await page
    .getByPlaceholder("Ask a question")
    .fill("Create and read note.txt");
  await page.getByRole("button", { name: "Send message" }).click();

  await expect(page.getByText("Running command")).toBeVisible();
  await expect(page.getByText("Command completed")).toBeVisible();
  await expect(page.getByText("The file contains hello.")).toBeVisible();
  await page.getByRole("button", { name: "Command details" }).click();
  await expect(
    page.getByText("printf hello > note.txt; cat note.txt"),
  ).toBeVisible();
  await expect(page.getByText("hello", { exact: true })).toBeVisible();
});

test("shows failed, timed out, and truncated bash results", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [],
    turns: [
      { events: [assistant("", [bashCall("failed", "false")])] },
      {
        events: [
          assistant("", [bashCall("timed", "sleep 1", { timeout_ms: 250 })]),
        ],
      },
      {
        events: [
          assistant("", [
            bashCall("truncated", "printf '%300s' x | tr ' ' x", {
              max_output_chars: 256,
            }),
          ]),
        ],
      },
      { events: [assistant("Done")] },
    ],
  });

  await page.goto("/agent");
  await page.getByPlaceholder("Ask a question").fill("Exercise bash failures");
  await page.getByRole("button", { name: "Send message" }).click();

  await expect(page.getByText("Command failed")).toBeVisible();
  await expect(page.getByText("Command timed out")).toBeVisible();
  await page.getByRole("button", { name: "Command details" }).last().click();
  await expect(page.getByText("Output was truncated")).toBeVisible();
});

test("stops an active turn and restores the composer", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, { conversations: [], turns: [{ hold: true }] });

  await page.goto("/agent");
  await page.getByPlaceholder("Ask a question").fill("Wait forever");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(
    page.getByRole("button", { name: "Stop generating" }),
  ).toBeVisible();
  await expect(page.getByText("Thinking...", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Stop generating" }).click();
  await expect(
    page.getByRole("button", { name: "Stop generating" }),
  ).toHaveCount(0);
  await expect(page.getByPlaceholder("Ask a question")).toBeEnabled();
  await expect(page.getByText("Thinking...", { exact: true })).toHaveCount(0);
});

test("shows an API error and retries without duplicating the user bubble", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    conversations: [],
    turns: [
      {
        hold: true,
        error: { status: 500, message: "Backend unavailable" },
      },
      {
        events: [
          { type: "content_delta", delta: "Recovered." },
          assistant("Recovered"),
        ],
      },
    ],
  });

  await page.goto("/agent");
  await page.getByPlaceholder("Ask a question").fill("Try again");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(page.getByText("Thinking...", { exact: true })).toBeVisible();

  chat.releaseTurn();
  await expect(page.getByText("Thinking...", { exact: true })).toHaveCount(0);
  await expect(page.getByRole("alert")).toContainText("Backend unavailable");
  await page.getByRole("button", { name: "Retry" }).click();
  await expect(page.getByText("Recovered")).toBeVisible();
  await expect(page.getByText("Try again")).toHaveCount(1);
  expect(chat.turnRequests.map((request) => request.messages)).toEqual([
    [{ role: "user", content: "Try again" }],
    [],
  ]);
});

test("keeps streamed Markdown in one assistant bubble", async ({ page }) => {
  const markdown = "| State | Value |\n| --- | --- |\n| Stream | Ready |";
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [],
    turns: [
      {
        events: [
          { type: "content_delta", delta: markdown.slice(0, 24) },
          { type: "content_delta", delta: markdown.slice(24) },
          assistant(markdown),
        ],
      },
    ],
  });

  await page.goto("/agent");
  await page.getByPlaceholder("Ask a question").fill("Stream");
  await page.getByRole("button", { name: "Send message" }).click();
  const assistantBubble = page.getByLabel(/^Assistant at /);
  await expect(assistantBubble.getByRole("table")).toBeVisible();
  await expect(
    assistantBubble.getByRole("cell", { name: "Ready" }),
  ).toBeVisible();
  await expect(assistantBubble).toHaveCount(1);
});

test("reconstructs stored bash command steps from tool history", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [
      {
        id: "stored",
        title: "Stored command",
        created_at: now,
        updated_at: now,
        messages: [
          { id: "user", role: "user", content: "Read note", created_at: now },
          {
            id: "call",
            role: "assistant",
            content: "",
            tool_calls: [bashCall("stored-call", "cat note.txt")],
            created_at: now,
          },
          {
            id: "result",
            role: "tool",
            tool_call_id: "stored-call",
            content: JSON.stringify({
              stdout: "hello",
              stderr: "",
              exit_code: 0,
              timed_out: false,
              truncated: false,
            }),
            created_at: now,
          },
          {
            id: "answer",
            role: "assistant",
            content: "The note says hello.",
            created_at: now,
          },
        ],
      },
    ],
  });

  await page.goto("/agent/stored");
  await expect(page.getByText("Command completed")).toBeVisible();
  await page.getByText("Command details").click();
  await expect(page.getByText("cat note.txt")).toBeVisible();
  await expect(page.getByText("hello", { exact: true })).toBeVisible();
  await expect(page.getByText("The note says hello.")).toBeVisible();
});

test("reconciles persisted turn history without duplicate transient entries", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [],
    turns: [
      { events: [assistant("", [bashCall("once", "printf hello")])] },
      {
        events: [
          { type: "content_delta", delta: "Final once." },
          assistant("Final once"),
        ],
      },
    ],
  });
  await page.goto("/agent");
  await page.getByPlaceholder("Ask a question").fill("Run once");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(page.getByText("Final once", { exact: true })).toBeVisible();
  await expect(page.getByText("Run once", { exact: true })).toHaveCount(1);
  await expect(
    page.getByText("Command completed", { exact: true }),
  ).toHaveCount(1);
  await expect(page.getByText("Final once", { exact: true })).toHaveCount(1);
});

test("keeps successful transient output and refreshes without rerunning the model", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    conversations: [],
    refreshError: { afterTurnRequests: 1, message: "Refresh unavailable" },
    turns: [
      {
        events: [
          { type: "content_delta", delta: "Completed." },
          assistant("Completed"),
        ],
      },
    ],
  });
  await page.goto("/agent");
  await page.getByPlaceholder("Ask a question").fill("Complete then refresh");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(page.getByText("Completed")).toBeVisible();
  await expect(page.getByRole("alert")).toContainText("Refresh unavailable");
  await expect(
    page.getByRole("button", { name: "Refresh conversation" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Refresh conversation" }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  expect(chat.turnRequests).toHaveLength(1);
});

test("stops an open streamed response without showing generation retry", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, { conversations: [] });
  await page.addInitScript(() => {
    const originalFetch = window.fetch.bind(window);
    window.fetch = async (input, init) => {
      const url =
        typeof input === "string"
          ? input
          : input instanceof Request
            ? input.url
            : input.toString();
      if (!url.includes("/turns")) return originalFetch(input, init);
      (window as Window & { streamHeaders?: string }).streamHeaders =
        "x-stream-ready";
      const encoder = new TextEncoder();
      return new Response(
        new ReadableStream({
          start(controller) {
            controller.enqueue(
              encoder.encode(
                '{"type":"content_delta","delta":"Streaming now"}\n',
              ),
            );
            init?.signal?.addEventListener(
              "abort",
              () => controller.error(new DOMException("Aborted", "AbortError")),
              { once: true },
            );
          },
        }),
        { status: 200, headers: { "X-Stream-Ready": "yes" } },
      );
    };
  });
  await page.goto("/agent");
  await page.getByPlaceholder("Ask a question").fill("Stop streamed response");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(page.getByText("Streaming now")).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(
        () => (window as Window & { streamHeaders?: string }).streamHeaders,
      ),
    )
    .toBe("x-stream-ready");
  await page.getByRole("button", { name: "Stop generating" }).click();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(page.getByPlaceholder("Ask a question")).toBeEnabled();
});

test("announces conversation loading, loaded, and no selected conversation", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, { holdConversation: true });
  await page.goto("/agent/conversation-1");
  await expect(
    page.locator("[aria-live]").filter({ hasText: "Loading conversation" }),
  ).toBeAttached();
  chat.releaseConversation();
  await expect(
    page.locator("[aria-live]").filter({ hasText: "Conversation loaded" }),
  ).toBeAttached();
});

test("locks submission until post-turn refresh reconciliation completes", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    conversations: [],
    holdRefresh: true,
    turns: [
      {
        events: [
          { type: "content_delta", delta: "First final." },
          assistant("First final."),
        ],
      },
      {
        events: [
          { type: "content_delta", delta: "Second final." },
          assistant("Second final."),
        ],
      },
    ],
  });

  await page.goto("/agent");
  await page.getByPlaceholder("Ask a question").fill("First prompt");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(page.getByText("First final.", { exact: true })).toBeVisible();

  const prompt = page.getByPlaceholder("Ask a question");
  await prompt.fill("Second prompt");
  await expect(
    page.getByRole("button", { name: "Send message" }),
  ).toBeDisabled();
  await page
    .getByRole("button", { name: "Send message" })
    .click({ force: true });
  expect(chat.turnRequests).toHaveLength(1);

  chat.releaseRefresh();
  await expect(
    page.getByRole("button", { name: "Send message" }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(page.getByText("Second final.", { exact: true })).toBeVisible();
  expect(chat.turnRequests).toHaveLength(2);
});

test("serializes held refresh recovery before allowing another turn", async ({
  page,
}) => {
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    conversations: [],
    refreshError: { afterTurnRequests: 1, message: "Refresh unavailable" },
    holdRefresh: true,
    turns: [
      {
        events: [
          { type: "content_delta", delta: "Recovered first." },
          assistant("Recovered first."),
        ],
      },
      {
        events: [
          { type: "content_delta", delta: "Second after recovery." },
          assistant("Second after recovery."),
        ],
      },
    ],
  });

  await page.goto("/agent");
  await page.getByPlaceholder("Ask a question").fill("First recovery prompt");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(
    page.getByRole("button", { name: "Refresh conversation" }),
  ).toBeVisible();

  await page.getByRole("button", { name: "Refresh conversation" }).click();
  const prompt = page.getByPlaceholder("Ask a question");
  await prompt.fill("Blocked during recovery");
  await expect(
    page.getByRole("button", { name: "Send message" }),
  ).toBeDisabled();
  await expect(
    page.getByRole("button", { name: "Stop generating" }),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "Send message" })
    .click({ force: true });
  expect(chat.turnRequests).toHaveLength(1);

  chat.releaseRefresh();
  await expect(page.getByRole("alert")).toHaveCount(0);
  await expect(
    page.getByText("Recovered first.", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Send message" }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(
    page.getByText("Second after recovery.", { exact: true }),
  ).toBeVisible();
  expect(chat.turnRequests).toHaveLength(2);
});

test("locks native thread navigation until an active turn reconciles", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    conversations: [
      {
        id: "conversation-a",
        title: "Conversation A",
        created_at: now,
        updated_at: now,
        messages: [
          { id: "a-user", role: "user", content: "A history", created_at: now },
        ],
      },
      {
        id: "conversation-b",
        title: "Conversation B",
        created_at: now,
        updated_at: now,
        messages: [
          {
            id: "b-user",
            role: "user",
            content: "B only message",
            created_at: now,
          },
        ],
      },
    ],
    turns: [
      {
        hold: true,
        events: [
          { type: "content_delta", delta: "A final." },
          assistant("A final."),
        ],
      },
    ],
  });

  await page.goto("/agent/conversation-a");
  await expect(page.getByText("A history")).toBeVisible();
  await page.getByPlaceholder("Ask a question").fill("Run in A");
  await page.getByRole("button", { name: "Send message" }).click();
  await page.getByRole("link", { name: "Conversation B" }).click();
  await expect(page).toHaveURL(/\/agent\/conversation-a$/);
  await expect(page.getByText("Run in A", { exact: true })).toBeVisible();
  await expect(page.getByText("B only message")).toHaveCount(0);

  chat.releaseTurn();
  await expect(page.getByText("A final.", { exact: true })).toBeVisible();
  await page.getByRole("link", { name: "Conversation B" }).click();
  await expect(page).toHaveURL(/\/agent\/conversation-b$/);
  await expect(page.getByText("B only message")).toBeVisible();
});

test("clears pending feedback after a forced thread route change", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    conversations: [
      {
        id: "conversation-a",
        title: "Conversation A",
        created_at: now,
        updated_at: now,
        messages: [
          { id: "a-user", role: "user", content: "A history", created_at: now },
        ],
      },
      {
        id: "conversation-b",
        title: "Conversation B",
        created_at: now,
        updated_at: now,
        messages: [
          {
            id: "b-user",
            role: "user",
            content: "B only message",
            created_at: now,
          },
        ],
      },
    ],
    turns: [{ hold: true }],
  });

  await page.goto("/agent/conversation-a");
  const transcript = page.getByRole("region", { name: "Chat" });
  await page.getByPlaceholder("Ask a question").fill("Run in A");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(transcript.getByText("Thinking...", { exact: true })).toBeVisible();

  await page.evaluate(() => {
    const transcript = document.querySelector(".agent-chat-transcript");
    if (!transcript) throw new Error("Chat transcript was not rendered");
    const tracker: { observer: MutationObserver; sawPending: boolean } = {
      observer: undefined as unknown as MutationObserver,
      sawPending: false,
    };
    const observer = new MutationObserver((records) => {
      if (window.location.pathname !== "/agent/conversation-b") return;
      if (
        records.some((record) =>
          [...record.addedNodes].some(
            (node) =>
              node instanceof Element &&
              (node.matches(".agent-chat-thinking") ||
                node.querySelector(".agent-chat-thinking")),
          ),
        )
      )
        tracker.sawPending = true;
    });
    tracker.observer = observer;
    observer.observe(transcript, { childList: true, subtree: true });
    (window as typeof window & {
      pendingFeedbackRouteTracker?: typeof tracker;
    }).pendingFeedbackRouteTracker = tracker;
    window.history.pushState({}, "", "/agent/conversation-b");
    window.dispatchEvent(new PopStateEvent("popstate"));
  });

  await expect(page).toHaveURL(/\/agent\/conversation-b$/);
  await expect(transcript.getByText("B only message", { exact: true })).toBeVisible();
  await expect(transcript.getByText("Thinking...", { exact: true })).toHaveCount(0);
  await expect
    .poll(() =>
      page.evaluate(() => {
        const pageWindow = window as typeof window & {
          pendingFeedbackRouteTracker?: {
            observer: MutationObserver;
            sawPending: boolean;
          };
        };
        const tracker = pageWindow.pendingFeedbackRouteTracker;
        tracker?.observer.disconnect();
        return tracker?.sawPending ?? false;
      }),
    )
    .toBe(false);
  chat.releaseTurn();
});

test("pending New thread keeps the created summary without reclaiming navigation", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    holdCreate: true,
    conversations: [
      {
        id: "conversation-1",
        title: "Existing thread",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/conversation-1");
  await page.getByRole("link", { name: "New thread" }).click();
  await expect.poll(() => chat.createRequests).toBe(1);
  await page.goto("/pools");

  chat.releaseCreate();

  await expect(page).toHaveURL(/\/pools$/);
  await page.goto("/agent/conversation-1");
  await expect(page.getByRole("link", { name: "New conversation" })).toBeVisible();
});

test("uses New conversation for empty thread titles", async ({ page }) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [
      {
        id: "untitled-thread",
        title: "",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/untitled-thread");

  await expect(page.getByRole("link", { name: "New conversation" })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Actions for New conversation" }),
  ).toBeVisible();
});

test("archiving the last thread moves to archived safety if replacement creation fails", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    createError: { message: "Replacement failed" },
    conversations: [
      {
        id: "last-thread",
        title: "Last active thread",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/last-thread");
  await page
    .getByRole("button", { name: "Actions for Last active thread" })
    .click();
  await page.getByRole("menuitem", { name: "Archive" }).click();

  await expect
    .poll(() => chat.patchRequests)
    .toEqual([{ conversationId: "last-thread", archived: true }]);
  await expect(page).toHaveURL(/\/agent\/archived$/);
  await expect(page.getByPlaceholder("Ask a question")).toHaveCount(0);
});

test("unknown thread recovery opens the newest shared active thread", async ({
  page,
}) => {
  const now = Date.now();
  await mockAuth(page, { admin: false, chat: true });
  await mockChatApi(page, {
    conversations: [
      {
        id: "older-thread",
        title: "Older thread",
        created_at: new Date(now - 120_000).toISOString(),
        updated_at: new Date(now - 120_000).toISOString(),
        messages: [],
      },
      {
        id: "newest-thread",
        title: "Newest thread",
        created_at: new Date(now - 60_000).toISOString(),
        updated_at: new Date(now - 60_000).toISOString(),
        messages: [],
      },
    ],
  });

  await page.goto("/agent/missing-thread");
  await expect(page.getByText("conversation not found")).toBeVisible();
  await page.getByRole("button", { name: "Open newest thread" }).click();

  await expect(page).toHaveURL(/\/agent\/newest-thread$/);
});

test("archiving the final thread opens its successful replacement", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    holdCreate: true,
    conversations: [
      {
        id: "last-thread",
        title: "Last active thread",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/last-thread");
  await page
    .getByRole("button", { name: "Actions for Last active thread" })
    .click();
  await page.getByRole("menuitem", { name: "Archive" }).click();
  await expect(page).toHaveURL(/\/agent\/archived$/);
  await expect.poll(() => chat.createRequests).toBe(1);

  chat.releaseCreate();

  await expect(page).toHaveURL(/\/agent\/conversation-2$/);
  await expect(page.getByRole("link", { name: "New conversation" })).toBeVisible();
});

test("a pending create prevents a turn and opens its created thread", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    holdCreate: true,
    conversations: [
      {
        id: "conversation-1",
        title: "Existing thread",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/conversation-1");
  await page.getByRole("link", { name: "New thread" }).click();
  await expect.poll(() => chat.createRequests).toBe(1);
  await expect(page.getByPlaceholder("Ask a question")).toBeDisabled();
  await expect(page.getByRole("button", { name: "Send message" })).toBeDisabled();
  await expect.poll(() => chat.turnRequests.length).toBe(0);

  chat.releaseCreate();

  await expect(page).toHaveURL(/\/agent\/conversation-2$/);
  await expect(page.getByRole("link", { name: "New conversation" })).toBeVisible();
});

test("a pending archive prevents a turn", async ({ page }) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    holdPatch: true,
    conversations: [
      {
        id: "conversation-1",
        title: "Archive target",
        created_at: now,
        updated_at: now,
        messages: [],
      },
      {
        id: "conversation-2",
        title: "Fallback thread",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
  });

  await page.goto("/agent/conversation-1");
  await page.getByPlaceholder("Ask a question").fill("Do not send this");
  await page
    .getByRole("button", { name: "Actions for Archive target" })
    .click();
  await page.getByRole("menuitem", { name: "Archive" }).click();
  await expect.poll(() => chat.patchRequests.length).toBe(1);
  await expect(page.getByPlaceholder("Ask a question")).toBeDisabled();
  await expect(page.getByRole("button", { name: "Send message" })).toBeDisabled();
  await expect.poll(() => chat.turnRequests.length).toBe(0);

  chat.releasePatch();
  await expect(page).toHaveURL(/\/agent\/conversation-2$/);
});

test("Undo is disabled while generating and restores after generation", async ({
  page,
}) => {
  const now = new Date().toISOString();
  await mockAuth(page, { admin: false, chat: true });
  const chat = await mockChatApi(page, {
    conversations: [
      {
        id: "conversation-1",
        title: "Archive target",
        created_at: now,
        updated_at: now,
        messages: [],
      },
      {
        id: "conversation-2",
        title: "Generation target",
        created_at: now,
        updated_at: now,
        messages: [],
      },
    ],
    turns: [
      {
        hold: true,
        events: [
          {
            type: "assistant",
            message: { role: "assistant", content: "Generation complete" },
          },
        ],
      },
    ],
  });

  await page.goto("/agent/conversation-1");
  await page
    .getByRole("button", { name: "Actions for Archive target" })
    .click();
  await page.getByRole("menuitem", { name: "Archive" }).click();
  await expect(page).toHaveURL(/\/agent\/conversation-2$/);

  await page.getByPlaceholder("Ask a question").fill("Hold this turn");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect.poll(() => chat.turnRequests.length).toBe(1);
  await expect(page.getByRole("button", { name: "Undo" })).toBeDisabled();

  chat.releaseTurn();
  await expect(page.getByRole("button", { name: "Undo" })).toBeEnabled();
  await page.getByRole("button", { name: "Undo" }).click();
  await expect
    .poll(() => chat.patchRequests)
    .toEqual([
      { conversationId: "conversation-1", archived: true },
      { conversationId: "conversation-1", archived: false },
    ]);
});
