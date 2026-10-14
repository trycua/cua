import { expect, test, type Page } from "@playwright/test";
import {
  mockChatApi,
  type MockChatConversation,
} from "./fixtures/mock-api";

async function request(
  page: Page,
  path: string,
  init: { body?: string; method?: string } = {},
): Promise<{ body: unknown; status: number }> {
  return page.evaluate(
    async ({ path, init }) => {
      const response = await fetch(path, {
        ...init,
        headers: init.body ? { "Content-Type": "application/json" } : undefined,
      });
      return { body: await response.json(), status: response.status };
    },
    { path, init },
  );
}

test("mock chat API enforces archive contract", async ({ page }) => {
  const now = "2026-08-16T20:20:00.000Z";
  const conversations: MockChatConversation[] = [
    {
      id: "active-conversation",
      title: "Active conversation",
      created_at: now,
      updated_at: now,
      messages: [],
    },
    {
      id: "archived-conversation",
      title: "Archived conversation",
      created_at: now,
      updated_at: now,
      archived_at: now,
      messages: [],
    },
  ];

  await page.route("http://mock.test/", (route) =>
    route.fulfill({ body: "<main>mock</main>" }),
  );
  const chat = await mockChatApi(page, {
    conversations,
    turns: [
      {
        events: [
          {
            type: "assistant",
            message: { role: "assistant", content: "unused" },
          },
        ],
      },
    ],
  });
  await page.goto("http://mock.test/");

  const active = await request(page, "/api/chat/conversations");
  expect(active.status).toBe(200);
  expect((active.body as Array<{ id: string }>).map((item) => item.id)).toEqual([
    "active-conversation",
  ]);

  const archived = await request(
    page,
    "/api/chat/conversations?archived=true",
  );
  expect(archived.status).toBe(200);
  expect(
    (archived.body as Array<{ id: string }>).map((item) => item.id),
  ).toEqual(["archived-conversation"]);

  for (const path of [
    "/api/chat/conversations?archived=",
    "/api/chat/conversations?archived=invalid",
    "/api/chat/conversations?archived=true&archived=true",
    "/api/chat/conversations?archived=true&archived=false",
  ]) {
    const response = await request(page, path);
    expect(response.status, `${path} is rejected`).toBe(400);
  }

  for (const body of [
    "not-json",
    "[]",
    "{}",
    '{"archived":true,"extra":false}',
    '{"archived":"true"}',
  ]) {
    const response = await request(
      page,
      "/api/chat/conversations/active-conversation",
      { method: "PATCH", body },
    );
    expect(response.status, `PATCH body ${body} is rejected`).toBe(400);
  }
  expect(chat.patchRequests).toHaveLength(0);

  const archive = await request(
    page,
    "/api/chat/conversations/active-conversation",
    { method: "PATCH", body: '{"archived":true}' },
  );
  expect(archive.status).toBe(200);
  expect((archive.body as { archived_at?: string }).archived_at).toBe(
    (archive.body as { updated_at: string }).updated_at,
  );
  expect(chat.patchRequests).toEqual([
    { conversationId: "active-conversation", archived: true },
  ]);

  const turn = await request(
    page,
    "/api/chat/conversations/active-conversation/turns",
    {
      method: "POST",
      body: '{"messages":[{"role":"user","content":"must not persist"}]}',
    },
  );
  expect(turn.status).toBe(409);
  expect(conversations[0].messages).toHaveLength(0);
  expect(conversations[0].updated_at).toBe(
    (archive.body as { updated_at: string }).updated_at,
  );
  expect(chat.remainingTurns).toBe(1);

  const restore = await request(
    page,
    "/api/chat/conversations/active-conversation",
    { method: "PATCH", body: '{"archived":false}' },
  );
  expect(restore.status).toBe(200);
  expect((restore.body as { archived_at?: string }).archived_at).toBeUndefined();
  expect(chat.patchRequests).toEqual([
    { conversationId: "active-conversation", archived: true },
    { conversationId: "active-conversation", archived: false },
  ]);
});
