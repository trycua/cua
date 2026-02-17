import type { PromptContext } from './prompt-analyzer';

interface PostHogEvent {
  properties: {
    prompt?: string;
    category?: string;
    question_type?: string;
    topics?: string[];
  };
}

interface PostHogEventsResponse {
  results: PostHogEvent[];
  next?: string;
}

export async function fetchRecentPrompts(hoursAgo: number = 1): Promise<PromptContext[]> {
  const apiKey = process.env.POSTHOG_PERSONAL_API_KEY;
  const projectId = process.env.POSTHOG_PROJECT_ID;
  const host = process.env.POSTHOG_HOST || 'https://us.i.posthog.com';

  if (!apiKey || !projectId) return [];

  const after = new Date(Date.now() - hoursAgo * 60 * 60 * 1000).toISOString();
  const prompts: PromptContext[] = [];
  let url: string | null =
    `${host}/api/projects/${projectId}/events/?event=copilot_user_prompt&after=${after}&limit=100`;

  while (url) {
    const response = await fetch(url, {
      headers: { Authorization: `Bearer ${apiKey}` },
    });

    if (!response.ok) {
      console.error(
        `[PostHogQuery] Failed to fetch events: ${response.status} ${response.statusText}`
      );
      break;
    }

    const data: PostHogEventsResponse = await response.json();

    for (const event of data.results) {
      const props = event.properties;
      if (!props.prompt) continue;

      prompts.push({
        prompt: props.prompt,
        category: props.category || 'other',
        questionType: props.question_type || 'other',
        topics: Array.isArray(props.topics) ? props.topics : [],
      });
    }

    url = data.next || null;
  }

  return prompts;
}
