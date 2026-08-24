import { expect, test } from "@playwright/test"

import { mockAuth, mockChatApi } from "./fixtures/mock-api"
import { expectSharedPageShell } from "./fixtures/shell-geometry"

test.describe("shared Chat page shell", () => {
  for (const viewport of [
    { name: "desktop", width: 1440, height: 900 },
    { name: "mobile", width: 390, height: 844 },
  ]) {
    test(`keeps the shared shell and composer in view on ${viewport.name}`, async ({
      page,
    }) => {
      await page.setViewportSize(viewport)
      await mockAuth(page, { admin: false, chat: true })
      await mockChatApi(page)
      await page.goto("/agent")

      await expectSharedPageShell(page)
      await expect(page.getByRole("heading", { name: "Chat" })).toBeVisible()
      await expect(
        page.getByText("Run fleet and browser tasks with the Cua agent."),
      ).toHaveCount(viewport.width > 700 ? 1 : 0)
      await expect(page.locator(".agent-chat-composer")).toBeInViewport()
      const composerBox = await page.locator(".agent-chat-composer").boundingBox()
      expect(composerBox).not.toBeNull()
      expect(
        viewport.height - ((composerBox?.y ?? 0) + (composerBox?.height ?? 0)),
      ).toBeLessThan(40)
      expect(
        await page.evaluate(
          () => document.documentElement.scrollHeight - window.innerHeight,
        ),
      ).toBeLessThan(4)

      if (viewport.width > 700) {
        await expect(page.locator(".agent-chat-timestamp").first()).toHaveCSS(
          "color",
          "rgba(164, 173, 187, 0.82)",
        )
        const geometry = await page.locator(".cua-pagehead").evaluate(
          async element => {
            const samples: string[] = []
            for (let frame = 0; frame < 30; frame += 1) {
              await new Promise<void>(resolve =>
                requestAnimationFrame(() => resolve()),
              )
              const box = element.getBoundingClientRect()
              samples.push(`${box.x}:${box.y}:${box.width}:${box.height}`)
            }
            return samples
          },
        )
        expect(new Set(geometry).size).toBe(1)
      }
    })
  }
})

test("renders and exercises Chat through the standalone visual preview", async ({
  page,
}) => {
  await page.goto("/agent?cua-visual-preview")

  await expect(page).toHaveTitle("Chat · Cua")
  await expect(page.getByRole("heading", { name: "Chat" })).toBeVisible()
  await expect(page.getByRole("link", { name: "Chat" })).toBeVisible()
  await page
    .getByRole("button", { name: "Inspect the browser fleet" })
    .click()
  await expect(
    page.getByText("Check the browser fleet and summarize its availability."),
  ).toBeVisible()
  await expect(page.getByText("Command completed")).toBeVisible()
  await expect(
    page.getByText("The browser fleet is healthy:", { exact: false }),
  ).toBeVisible()

  const prompt = page.getByPlaceholder("Ask a question")
  await prompt.fill("Show me the preview response")
  await page.getByRole("button", { name: "Send message" }).click()
  await expect(
    page.getByText("Preview mode is connected.", { exact: false }),
  ).toBeVisible()
  await expect(prompt).toBeEnabled()
})

test("offers a useful first-run Chat state in the standalone preview", async ({
  page,
}) => {
  await page.goto("/agent?cua-visual-preview&cua-preview-state=empty")

  await expect(
    page.getByRole("heading", { name: "Start a conversation" }),
  ).toBeVisible()
  await expect(
    page.getByText("Ask a question below to create your first conversation."),
  ).toBeVisible()
  await expect(page.getByPlaceholder("Ask a question")).toBeEnabled()
})

test("shows chat history, transcript region, prompt shell, and authenticated history time", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  const chat = await mockChatApi(page)

  await page.goto("/agent")

  await expect(page.getByRole("heading", { name: "Conversations" })).toBeVisible()
  await expect(page.getByText("Today")).toBeVisible()
  await expect(page.getByRole("region", { name: "Chat" })).toBeVisible()
  await expect(page.getByPlaceholder("Ask a question")).toBeVisible()
  await expect(page.locator(".agent-chat-history [title]").first()).toHaveAttribute(
    "title",
    /.+/,
  )
  expect(chat.authorizationHeaders.length).toBeGreaterThan(0)
  expect(chat.authorizationHeaders).toEqual(
    expect.arrayContaining([expect.stringMatching(/^Bearer /)]),
  )
})

test("renders a full-width multiline composer with hidden announcements", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page)

  await page.goto("/agent")

  const composer = page.locator(".agent-chat-composer")
  const prompt = page.locator(".agent-chat-prompt")
  const textarea = page.getByPlaceholder("Ask a question")
  const composerBox = await composer.boundingBox()
  const promptBox = await prompt.boundingBox()
  const initialHeight = (await textarea.boundingBox())?.height ?? 0

  expect(composerBox).not.toBeNull()
  expect(promptBox).not.toBeNull()
  expect((promptBox?.width ?? 0) / (composerBox?.width ?? 1)).toBeGreaterThan(0.9)
  expect(initialHeight).toBeGreaterThan(44)

  await textarea.fill("First line\nSecond line\nThird line")
  await expect.poll(async () => (await textarea.boundingBox())?.height ?? 0).toBeGreaterThan(initialHeight)

  const announcement = page.locator('[aria-live="polite"]').filter({ hasText: "No conversation selected" })
  const announcements = page.locator(".agent-chat-announcements")
  await expect(announcement).toBeAttached()
  await expect(announcements).toHaveCSS("overflow", "hidden")
  await expect.poll(async () => announcements.evaluate(element => ({ width: element.clientWidth, height: element.clientHeight }))).toEqual({ width: 1, height: 1 })
})

test("keeps the prompt visible while conversation history loads", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  const chat = await mockChatApi(page, { holdList: true })

  await page.goto("/agent")

  await expect(page.getByTestId("conversation-skeleton")).toHaveCount(3)
  await expect(page.getByPlaceholder("Ask a question")).toBeVisible()
  chat.releaseList()
})

test("loads a selected conversation with message skeletons and accessible author times", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  const chat = await mockChatApi(page, { holdConversation: true })

  await page.goto("/agent")
  await page.getByRole("button", { name: "Example browser task" }).click()

  await expect(page.getByTestId("message-skeleton")).toHaveCount(2)
  await expect(page.getByPlaceholder("Ask a question")).toBeVisible()
  chat.releaseConversation()

  await expect(page.getByText("Open the example site.")).toBeVisible()
  await expect(page.getByText("The example site is ready.")).toBeVisible()
  await expect(page.getByLabel("Your avatar")).toHaveText("Y")
  await expect(page.getByLabel(/^You at /)).toBeVisible()
  await expect(page.getByLabel(/^Assistant at /)).toBeVisible()
})

test("renders and sanitizes Markdown in user and assistant messages", async ({ page }) => {
  const now = new Date().toISOString()
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page, {
    conversations: [{
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
            "| Pool | Phase |",
            "| --- | --- |",
            "| demo | Ready |",
            "",
            "```ts",
            "const ready = true",
            "```",
          ].join("\n"),
          created_at: now,
        },
      ],
    }],
  })

  await page.goto("/agent")
  await page.getByRole("button", { name: "Markdown chat" }).click()

  const userBubble = page.getByLabel(/^You at /)
  const assistantBubble = page.getByLabel(/^Assistant at /)
  await expect(userBubble.getByRole("table")).toBeVisible()
  await expect(userBubble.getByRole("link", { name: "Safe link" })).toHaveAttribute("href", "https://example.com")
  await expect(userBubble.getByRole("link", { name: "Safe link" })).toHaveCSS(
    "color",
    "rgb(159, 215, 255)",
  )
  await expect(userBubble.getByText("Unsafe link")).not.toHaveAttribute("href", /.+/)
  await expect(assistantBubble.getByRole("table")).toBeVisible()
  await expect(assistantBubble.locator("pre code")).toContainText("const ready = true")

  await expect(userBubble.locator("script, img")).toHaveCount(0)
  const sanitizedSpan = userBubble.getByText("Sanitized text")
  await expect(sanitizedSpan).not.toHaveAttribute("style")
  await expect(sanitizedSpan).not.toHaveAttribute("onclick")
  await expect(sanitizedSpan).not.toHaveAttribute("id")
  expect(await page.evaluate(() => (window as Window & { __markdownXss?: boolean }).__markdownXss)).toBeUndefined()
})

test("does not redirect while chat feature configuration is pending", async ({ page }) => {
  const auth = await mockAuth(page, { admin: false, chat: true }, { holdConfig: true })
  await mockChatApi(page)

  await page.goto("/agent")
  await expect(page).toHaveURL(/\/agent$/)
  await expect(page.getByRole("heading", { name: "Conversations" })).toHaveCount(0)

  auth.releaseConfig()
  await expect(page.getByRole("heading", { name: "Conversations" })).toBeVisible()
})

test("manages mobile history modal focus, keyboard, inert background, and close restoration", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page)

  await page.goto("/agent")

  const trigger = page.getByRole("button", { name: "View conversations" })
  await expect(trigger).toBeVisible()
  await expect(page.getByRole("dialog", { name: "Conversations" })).toHaveCount(0)

  await trigger.focus()
  await trigger.press("Enter")
  const dialog = page.getByRole("dialog", { name: "Conversations" })
  const drawer = dialog.locator(".agent-chat-mobile-drawer")
  const closeButton = dialog.getByRole("button", { name: "Close conversations" })
  await expect(dialog).toBeVisible()
  await expect(drawer).toHaveCSS("background-color", "rgb(24, 24, 24)")
  await expect(closeButton).toBeFocused()
  await expect(dialog.getByRole("heading", { name: "Conversations", exact: true })).toHaveCount(1)
  await expect(page.locator("#root")).toHaveAttribute("inert", "")
  const cuaIdentity = page.getByRole("link", { name: "Cua", exact: true }).first()
  await cuaIdentity.focus()
  await expect(cuaIdentity).not.toBeFocused()
  await expect(closeButton).toBeFocused()

  const lastFocusable = dialog.locator('button:not([disabled])').last()
  await closeButton.press("Shift+Tab")
  await expect(lastFocusable).toBeFocused()
  await lastFocusable.press("Tab")
  await expect(closeButton).toBeFocused()

  await page.keyboard.press("Escape")
  await expect(page.getByRole("dialog", { name: "Conversations" })).toHaveCount(0)
  await expect(trigger).toBeFocused()
  await expect(page.locator("#root")).not.toHaveAttribute("inert", "")

  await trigger.click()
  await page.getByTestId("conversation-history-backdrop").click({ position: { x: 380, y: 420 } })
  await expect(page.getByRole("dialog", { name: "Conversations" })).toHaveCount(0)
  await expect(trigger).toBeFocused()

  await trigger.click()
  await closeButton.click()
  await expect(page.getByRole("dialog", { name: "Conversations" })).toHaveCount(0)
  await expect(trigger).toBeFocused()
})

test("selects mobile history without horizontal page overflow", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 })
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page)

  await page.goto("/agent")
  await page.getByRole("button", { name: "View conversations" }).click()
  await page.getByRole("dialog", { name: "Conversations" }).getByRole("button", { name: "Example browser task" }).click()

  await expect(page.getByText("The example site is ready.")).toBeVisible()
  await expect(page.getByRole("dialog", { name: "Conversations" })).toHaveCount(0)
  await expect.poll(() => page.evaluate(() => ({
    body: document.body.scrollWidth <= document.body.clientWidth,
    document: document.documentElement.scrollWidth <= document.documentElement.clientWidth,
  }))).toEqual({ body: true, document: true })
})

test("returns focus to the prompt after keyboard submission completes", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page, {
    conversations: [],
    turns: [{ events: [{ type: "content_delta", delta: "Keyboard complete." }, assistant("Keyboard complete.")] }],
  })

  await page.goto("/agent")
  const prompt = page.getByPlaceholder("Ask a question")
  await prompt.fill("Submit from keyboard")
  await prompt.press("Enter")

  await expect(page.getByText("Keyboard complete.", { exact: true })).toBeVisible()
  await expect(page.locator('[aria-live="polite"]').filter({ hasText: /^Latest assistant message available\.$/ })).toHaveCount(1)
  await expect(page.locator('[aria-live]').filter({ hasText: /Latest user message available/ })).toHaveCount(0)
  await expect(prompt).toBeEnabled()
  await expect(prompt).toBeFocused()
})

test("exposes chat states and controls by accessible role and unique name", async ({ page }) => {
  const now = new Date().toISOString()
  await mockAuth(page, { admin: false, chat: true })
  const chat = await mockChatApi(page, {
    conversations: [{
      id: "accessible-chat",
      title: "Accessible chat",
      created_at: now,
      updated_at: now,
      messages: [
        { id: "user-one", role: "user", content: "First question", created_at: now },
        { id: "user-two", role: "user", content: "Second question", created_at: now },
      ],
    }],
    turns: [
      { hold: true, events: [assistant("", [bashCall("accessible-command", "printf accessible")])] },
      { error: { status: 500, message: "Accessible failure" } },
    ],
  })

  await page.goto("/agent")
  await page.getByRole("button", { name: "Accessible chat" }).click()
  const transcript = page.getByRole("region", { name: "Chat" })
  await expect(transcript).toBeVisible()
  const bubbleLabels = await transcript.locator("[aria-label]").evaluateAll(elements =>
    elements.map(element => element.getAttribute("aria-label")).filter(label => label?.startsWith("You at ")),
  )
  expect(new Set(bubbleLabels).size).toBe(bubbleLabels.length)

  const prompt = page.getByPlaceholder("Ask a question")
  await prompt.fill("Run accessible command")
  await page.getByRole("button", { name: "Send message" }).click()
  await expect(page.locator('[aria-live="polite"]').filter({ hasText: "Generating response" })).toHaveCount(1)
  await expect(transcript.locator(':scope > [role="status"]')).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Stop generating" })).toBeVisible()
  chat.releaseTurn()
  await expect(page.getByRole("button", { name: "Command details" })).toBeVisible()
  await expect(page.getByRole("alert")).toContainText("Accessible failure")
  await expect(page.locator('[aria-live]').filter({ hasText: "Accessible failure" })).toHaveCount(0)
})

test("scrolls the newest message into view while keeping the composer fixed", async ({ page }) => {
  const now = new Date().toISOString()
  const messages = Array.from({ length: 24 }, (_, index) => ({
    id: `overflow-${index}`,
    role: index % 2 === 0 ? "user" as const : "assistant" as const,
    content: `Overflow message ${index + 1} with enough text to occupy transcript space.`,
    created_at: now,
  }))
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page, {
    conversations: [{ id: "overflow-chat", title: "Overflow chat", created_at: now, updated_at: now, messages }],
    turns: [{ events: [{ type: "content_delta", delta: "Newest response." }, assistant("Newest response.")] }],
  })

  await page.goto("/agent")
  await page.getByRole("button", { name: "Overflow chat" }).click()
  const transcript = page.getByRole("region", { name: "Chat" })
  const composer = page.locator(".agent-chat-composer")
  const composerBefore = await composer.boundingBox()
  await transcript.evaluate(element => { element.scrollTop = 0 })

  await page.getByPlaceholder("Ask a question").fill("Show the newest response")
  await page.getByRole("button", { name: "Send message" }).click()

  await expect(page.getByText("Newest response.", { exact: true })).toBeInViewport()
  await expect.poll(() => transcript.evaluate(element => element.scrollTop)).toBeGreaterThan(0)
  const composerAfter = await composer.boundingBox()
  expect(composerBefore).not.toBeNull()
  expect(composerAfter).not.toBeNull()
  expect(composerAfter?.y).toBeCloseTo(composerBefore?.y ?? 0, 0)
})

test("contains long chat history without inflating document scroll", async ({ page }) => {
  await page.setViewportSize({ width: 1920, height: 1080 })
  const now = new Date().toISOString()
  const messages = Array.from({ length: 20 }, (_, index) => ({
    id: `windows-history-${index}`,
    role: index % 2 === 0 ? "user" as const : "assistant" as const,
    content: `| Column | Value |\n| --- | --- |\n| Row ${index} | ${"windows-image-value ".repeat(8)} |`,
    created_at: now,
  }))
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page, {
    conversations: [{ id: "windows-history", title: "Windows history", created_at: now, updated_at: now, messages }],
  })

  await page.goto("/agent")
  await page.getByRole("button", { name: "Windows history" }).click()

  const transcript = page.getByRole("region", { name: "Chat" })
  await expect.poll(() => transcript.evaluate(element => element.scrollHeight - element.clientHeight)).toBeGreaterThan(1000)
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollHeight - window.innerHeight)).toBeLessThan(100)
})

test("parses fragmented unterminated NDJSON and rejects malformed final events", async ({ page }) => {
  await mockAuth(page)
  await page.goto("/pools")

  const result = await page.evaluate(async () => {
    const api = await import("/src/sdk/chat.ts")
    const originalFetch = window.fetch
    const encode = (value: string) => new TextEncoder().encode(value)
    const stream = (chunks: string[]) =>
      new ReadableStream<Uint8Array>({
        start(controller) {
          for (const chunk of chunks) controller.enqueue(encode(chunk))
          controller.close()
        },
      })

    try {
      window.fetch = async () =>
        new Response(
          stream([
            '{"type":"content_delta","delta":"Hel',
            'lo"}\n{"type":"assistant","message":{"role":"assistant","content":"Complete"}}',
          ]),
          { status: 200 },
        )
      const deltas: string[] = []
      const complete = await api.streamTurn("conversation-1", [], delta => deltas.push(delta))

      window.fetch = async () =>
        new Response(
          stream([
            '{"type":"assistant","message":{"role":"assistant","content":"ok","tool_calls":{}}}',
          ]),
          { status: 200 },
        )
      let malformedError = ""
      try {
        await api.streamTurn("conversation-1", [], () => undefined)
      } catch (error) {
        malformedError = error instanceof Error ? error.name : String(error)
      }

      return { complete, deltas, malformedError }
    } finally {
      window.fetch = originalFetch
    }
  })

  expect(result.deltas).toEqual(["Hello"])
  expect(result.complete.content).toBe("Complete")
  expect(result.malformedError).toBe("ChatApiError")
})

test("hides chat navigation and redirects when the feature is disabled", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: false })

  await page.goto("/pools")
  await expect(page.getByRole("link", { name: "Chat" })).toHaveCount(0)

  await page.goto("/agent")
  await expect(page).toHaveURL(/\/pools$/)
})

test("disables skeleton shimmer when reduced motion is requested", async ({ page }) => {
  await page.emulateMedia({ reducedMotion: "reduce" })
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page, { holdList: true })

  await page.goto("/agent")

  await expect(page.getByTestId("conversation-skeleton").first()).toBeVisible()
  await expect(page.getByTestId("conversation-skeleton").first()).toHaveCSS(
    "animation-name",
    "none",
  )
})

const bashCall = (id: string, command: string, options: Record<string, number> = {}) => ({
  id,
  type: "function" as const,
  function: { name: "bash", arguments: JSON.stringify({ command, ...options }) },
})

const assistant = (content: string, toolCalls: ReturnType<typeof bashCall>[] = []) => ({
  type: "assistant",
  message: { role: "assistant", content, tool_calls: toolCalls },
})

test("runs a browser bash tool loop and shows completed command details", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page, {
    conversations: [],
    turns: [
      { events: [assistant("", [bashCall("bash-1", "printf hello > note.txt; cat note.txt")])] },
      { events: [{ type: "content_delta", delta: "The file contains hello." }, assistant("The file contains hello.")] },
    ],
  })

  await page.goto("/agent")
  await page.getByPlaceholder("Ask a question").fill("Create and read note.txt")
  await page.getByRole("button", { name: "Send message" }).click()

  await expect(page.getByText("Running command")).toBeVisible()
  await expect(page.getByText("Command completed")).toBeVisible()
  await expect(page.getByText("The file contains hello.")).toBeVisible()
  await page.getByRole("button", { name: "Command details" }).click()
  await expect(page.getByText("printf hello > note.txt; cat note.txt")).toBeVisible()
  await expect(page.getByText("hello", { exact: true })).toBeVisible()
})

test("shows failed, timed out, and truncated bash results", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page, {
    conversations: [],
    turns: [
      { events: [assistant("", [bashCall("failed", "false")])] },
      { events: [assistant("", [bashCall("timed", "sleep 1", { timeout_ms: 250 })])] },
      { events: [assistant("", [bashCall("truncated", "printf '%300s' x | tr ' ' x", { max_output_chars: 256 })])] },
      { events: [assistant("Done")] },
    ],
  })

  await page.goto("/agent")
  await page.getByPlaceholder("Ask a question").fill("Exercise bash failures")
  await page.getByRole("button", { name: "Send message" }).click()

  await expect(page.getByText("Command failed")).toBeVisible()
  await expect(page.getByText("Command timed out")).toBeVisible()
  await page.getByRole("button", { name: "Command details" }).last().click()
  await expect(page.getByText("Output was truncated")).toBeVisible()
})

test("stops an active turn and restores the composer", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page, { conversations: [], turns: [{ hold: true }] })

  await page.goto("/agent")
  await page.getByPlaceholder("Ask a question").fill("Wait forever")
  await page.getByRole("button", { name: "Send message" }).click()
  await expect(page.getByRole("button", { name: "Stop generating" })).toBeVisible()
  await page.getByRole("button", { name: "Stop generating" }).click()
  await expect(page.getByRole("button", { name: "Stop generating" })).toHaveCount(0)
  await expect(page.getByPlaceholder("Ask a question")).toBeEnabled()
  await expect(page.getByText("Generating a response")).toHaveCount(0)
})

test("shows an API error and retries without duplicating the user bubble", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  const chat = await mockChatApi(page, {
    conversations: [],
    turns: [
      { error: { status: 500, message: "Backend unavailable" } },
      { events: [{ type: "content_delta", delta: "Recovered." }, assistant("Recovered")] },
    ],
  })

  await page.goto("/agent")
  await page.getByPlaceholder("Ask a question").fill("Try again")
  await page.getByRole("button", { name: "Send message" }).click()
  await expect(page.getByRole("alert")).toContainText("Backend unavailable")
  await page.getByRole("button", { name: "Retry" }).click()
  await expect(page.getByText("Recovered")).toBeVisible()
  await expect(page.getByText("Try again")).toHaveCount(1)
  expect(chat.turnRequests.map(request => request.messages)).toEqual([
    [{ role: "user", content: "Try again" }],
    [],
  ])
})

test("keeps streamed Markdown in one assistant bubble", async ({ page }) => {
  const markdown = "| State | Value |\n| --- | --- |\n| Stream | Ready |"
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page, {
    conversations: [],
    turns: [{ events: [{ type: "content_delta", delta: markdown.slice(0, 24) }, { type: "content_delta", delta: markdown.slice(24) }, assistant(markdown)] }],
  })

  await page.goto("/agent")
  await page.getByPlaceholder("Ask a question").fill("Stream")
  await page.getByRole("button", { name: "Send message" }).click()
  const assistantBubble = page.getByLabel(/^Assistant at /)
  await expect(assistantBubble.getByRole("table")).toBeVisible()
  await expect(assistantBubble.getByRole("cell", { name: "Ready" })).toBeVisible()
  await expect(assistantBubble).toHaveCount(1)
})

test("reconstructs stored bash command steps from tool history", async ({ page }) => {
  const now = new Date().toISOString()
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page, {
    conversations: [
      {
        id: "stored",
        title: "Stored command",
        created_at: now,
        updated_at: now,
        messages: [
          { id: "user", role: "user", content: "Read note", created_at: now },
          { id: "call", role: "assistant", content: "", tool_calls: [bashCall("stored-call", "cat note.txt")], created_at: now },
          { id: "result", role: "tool", tool_call_id: "stored-call", content: JSON.stringify({ stdout: "hello", stderr: "", exit_code: 0, timed_out: false, truncated: false }), created_at: now },
          { id: "answer", role: "assistant", content: "The note says hello.", created_at: now },
        ],
      },
    ],
  })

  await page.goto("/agent")
  await page.getByRole("button", { name: "Stored command" }).click()
  await expect(page.getByText("Command completed")).toBeVisible()
  await page.getByText("Command details").click()
  await expect(page.getByText("cat note.txt")).toBeVisible()
  await expect(page.getByText("hello", { exact: true })).toBeVisible()
  await expect(page.getByText("The note says hello.")).toBeVisible()
})

test("reconciles persisted turn history without duplicate transient entries", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page, {
    conversations: [],
    turns: [
      { events: [assistant("", [bashCall("once", "printf hello")])] },
      { events: [{ type: "content_delta", delta: "Final once." }, assistant("Final once")] },
    ],
  })
  await page.goto("/agent")
  await page.getByPlaceholder("Ask a question").fill("Run once")
  await page.getByRole("button", { name: "Send message" }).click()
  await expect(page.getByText("Final once", { exact: true })).toBeVisible()
  await expect(page.getByText("Run once", { exact: true })).toHaveCount(1)
  await expect(page.getByText("Command completed", { exact: true })).toHaveCount(1)
  await expect(page.getByText("Final once", { exact: true })).toHaveCount(1)
})

test("keeps successful transient output and refreshes without rerunning the model", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  const chat = await mockChatApi(page, {
    conversations: [],
    refreshError: { afterTurnRequests: 1, message: "Refresh unavailable" },
    turns: [{ events: [{ type: "content_delta", delta: "Completed." }, assistant("Completed")] }],
  })
  await page.goto("/agent")
  await page.getByPlaceholder("Ask a question").fill("Complete then refresh")
  await page.getByRole("button", { name: "Send message" }).click()
  await expect(page.getByText("Completed")).toBeVisible()
  await expect(page.getByRole("alert")).toContainText("Refresh unavailable")
  await expect(page.getByRole("button", { name: "Refresh conversation" })).toBeVisible()
  await page.getByRole("button", { name: "Refresh conversation" }).click()
  await expect(page.getByRole("alert")).toHaveCount(0)
  expect(chat.turnRequests).toHaveLength(1)
})


test("stops an open streamed response without showing generation retry", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  await mockChatApi(page, { conversations: [] })
  await page.addInitScript(() => {
    const originalFetch = window.fetch.bind(window)
    window.fetch = async (input, init) => {
      const url = typeof input === "string" ? input : input instanceof Request ? input.url : input.toString()
      if (!url.includes("/turns")) return originalFetch(input, init)
      ;(window as Window & { streamHeaders?: string }).streamHeaders = "x-stream-ready"
      const encoder = new TextEncoder()
      return new Response(new ReadableStream({
        start(controller) {
          controller.enqueue(encoder.encode('{"type":"content_delta","delta":"Streaming now"}\n'))
          init?.signal?.addEventListener("abort", () => controller.error(new DOMException("Aborted", "AbortError")), { once: true })
        },
      }), { status: 200, headers: { "X-Stream-Ready": "yes" } })
    }
  })
  await page.goto("/agent")
  await page.getByPlaceholder("Ask a question").fill("Stop streamed response")
  await page.getByRole("button", { name: "Send message" }).click()
  await expect(page.getByText("Streaming now")).toBeVisible()
  await expect.poll(() => page.evaluate(() => (window as Window & { streamHeaders?: string }).streamHeaders)).toBe("x-stream-ready")
  await page.getByRole("button", { name: "Stop generating" }).click()
  await expect(page.getByRole("alert")).toHaveCount(0)
  await expect(page.getByPlaceholder("Ask a question")).toBeEnabled()
})

test("announces conversation loading, loaded, and no selected conversation", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  const chat = await mockChatApi(page, { holdConversation: true })
  await page.goto("/agent")
  await expect(page.locator('[aria-live]').filter({ hasText: "No conversation selected" })).toBeAttached()
  await page.getByRole("button", { name: "Example browser task" }).click()
  await expect(page.locator('[aria-live]').filter({ hasText: "Loading conversation" })).toBeAttached()
  chat.releaseConversation()
  await expect(page.locator('[aria-live]').filter({ hasText: "Conversation loaded" })).toBeAttached()
})


test("locks submission until post-turn refresh reconciliation completes", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  const chat = await mockChatApi(page, {
    conversations: [],
    holdRefresh: true,
    turns: [
      { events: [{ type: "content_delta", delta: "First final." }, assistant("First final.")] },
      { events: [{ type: "content_delta", delta: "Second final." }, assistant("Second final.")] },
    ],
  })

  await page.goto("/agent")
  await page.getByPlaceholder("Ask a question").fill("First prompt")
  await page.getByRole("button", { name: "Send message" }).click()
  await expect(page.getByText("First final.", { exact: true })).toBeVisible()

  const prompt = page.getByPlaceholder("Ask a question")
  await prompt.fill("Second prompt")
  await expect(page.getByRole("button", { name: "Send message" })).toBeDisabled()
  await page.getByRole("button", { name: "Send message" }).click({ force: true })
  expect(chat.turnRequests).toHaveLength(1)

  chat.releaseRefresh()
  await expect(page.getByRole("button", { name: "Send message" })).toBeEnabled()
  await page.getByRole("button", { name: "Send message" }).click()
  await expect(page.getByText("Second final.", { exact: true })).toBeVisible()
  expect(chat.turnRequests).toHaveLength(2)
})


test("serializes held refresh recovery before allowing another turn", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  const chat = await mockChatApi(page, {
    conversations: [],
    refreshError: { afterTurnRequests: 1, message: "Refresh unavailable" },
    holdRefresh: true,
    turns: [
      { events: [{ type: "content_delta", delta: "Recovered first." }, assistant("Recovered first.")] },
      { events: [{ type: "content_delta", delta: "Second after recovery." }, assistant("Second after recovery.")] },
    ],
  })

  await page.goto("/agent")
  await page.getByPlaceholder("Ask a question").fill("First recovery prompt")
  await page.getByRole("button", { name: "Send message" }).click()
  await expect(page.getByRole("button", { name: "Refresh conversation" })).toBeVisible()

  await page.getByRole("button", { name: "Refresh conversation" }).click()
  const prompt = page.getByPlaceholder("Ask a question")
  await prompt.fill("Blocked during recovery")
  await expect(page.getByRole("button", { name: "Send message" })).toBeDisabled()
  await expect(page.getByRole("button", { name: "Stop generating" })).toHaveCount(0)
  await page.getByRole("button", { name: "Send message" }).click({ force: true })
  expect(chat.turnRequests).toHaveLength(1)

  chat.releaseRefresh()
  await expect(page.getByRole("alert")).toHaveCount(0)
  await expect(page.getByText("Recovered first.", { exact: true })).toBeVisible()
  await expect(page.getByRole("button", { name: "Send message" })).toBeEnabled()
  await page.getByRole("button", { name: "Send message" }).click()
  await expect(page.getByText("Second after recovery.", { exact: true })).toBeVisible()
  expect(chat.turnRequests).toHaveLength(2)
})


test("locks conversation selection until an active turn reconciles", async ({ page }) => {
  const now = new Date().toISOString()
  await mockAuth(page, { admin: false, chat: true })
  const chat = await mockChatApi(page, {
    conversations: [
      { id: "conversation-a", title: "Conversation A", created_at: now, updated_at: now, messages: [{ id: "a-user", role: "user", content: "A history", created_at: now }] },
      { id: "conversation-b", title: "Conversation B", created_at: now, updated_at: now, messages: [{ id: "b-user", role: "user", content: "B only message", created_at: now }] },
    ],
    turns: [{ hold: true, events: [{ type: "content_delta", delta: "A final." }, assistant("A final.")] }],
  })

  await page.goto("/agent")
  await page.getByRole("button", { name: "Conversation A" }).click()
  await expect(page.getByText("A history")).toBeVisible()
  await page.getByPlaceholder("Ask a question").fill("Run in A")
  await page.getByRole("button", { name: "Send message" }).click()
  await expect(page.getByRole("button", { name: "Conversation B" })).toBeDisabled()
  await page.getByRole("button", { name: "Conversation B" }).click({ force: true })
  await expect(page.getByText("Run in A", { exact: true })).toBeVisible()
  await expect(page.getByText("B only message")).toHaveCount(0)

  chat.releaseTurn()
  await expect(page.getByText("A final.", { exact: true })).toBeVisible()
  await expect(page.getByRole("button", { name: "Conversation B" })).toBeEnabled()
  await page.getByRole("button", { name: "Conversation B" }).click()
  await expect(page.getByText("B only message")).toBeVisible()
  await expect(page.getByText("A final.", { exact: true })).toHaveCount(0)
})


test("retries an initial conversation creation without stranding the lifecycle", async ({ page }) => {
  await mockAuth(page, { admin: false, chat: true })
  const chat = await mockChatApi(page, {
    conversations: [],
    createError: { message: "Conversation creation failed" },
    turns: [
      { events: [{ type: "content_delta", delta: "Created after retry." }, assistant("Created after retry.")] },
      { events: [{ type: "content_delta", delta: "Follow-up works." }, assistant("Follow-up works.")] },
    ],
  })

  await page.goto("/agent")
  await page.getByPlaceholder("Ask a question").fill("Create this conversation")
  await page.getByRole("button", { name: "Send message" }).click()
  await expect(page.getByRole("alert")).toContainText("Conversation creation failed")
  await expect(page.getByRole("button", { name: "Retry" })).toBeVisible()
  await expect(page.getByPlaceholder("Ask a question")).toBeEnabled()
  await expect(page.getByLabel(/^You at /).getByText("Create this conversation", { exact: true })).toHaveCount(0)
  expect(chat.createRequests).toBe(1)
  expect(chat.turnRequests).toHaveLength(0)

  await page.getByRole("button", { name: "Retry" }).click()
  await expect(page.getByText("Created after retry.", { exact: true })).toBeVisible()
  await expect(page.getByLabel(/^You at /).getByText("Create this conversation", { exact: true })).toHaveCount(1)
  expect(chat.createRequests).toBe(2)
  expect(chat.turnRequests).toHaveLength(1)
  expect(chat.turnRequests[0].messages).toEqual([{ role: "user", content: "Create this conversation" }])

  await page.getByPlaceholder("Ask a question").fill("Normal follow-up")
  await page.getByRole("button", { name: "Send message" }).click()
  await expect(page.getByText("Follow-up works.", { exact: true })).toBeVisible()
  expect(chat.turnRequests).toHaveLength(2)
})
