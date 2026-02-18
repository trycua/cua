import type { AnalyzedPrompt } from './prompt-analyzer';

const SCORE_THRESHOLD = 7;

function formatPromptBlock(item: AnalyzedPrompt): Record<string, unknown>[] {
  const { analysis, ctx } = item;
  const triggeredCriteria: string[] = [];

  if (analysis.actionability.score >= SCORE_THRESHOLD) {
    triggeredCriteria.push(
      `:wrench: *Actionability* (${analysis.actionability.score}/10): ${analysis.actionability.rationale}`
    );
  }
  if (analysis.tractionSignal.score >= SCORE_THRESHOLD) {
    triggeredCriteria.push(
      `:rocket: *Traction Signal* (${analysis.tractionSignal.score}/10): ${analysis.tractionSignal.rationale}`
    );
  }
  if (analysis.useCaseInsight.score >= SCORE_THRESHOLD) {
    triggeredCriteria.push(
      `:mag: *Use Case Insight* (${analysis.useCaseInsight.score}/10): ${analysis.useCaseInsight.rationale}`
    );
  }

  const topicsText = ctx.topics.length > 0 ? ctx.topics.join(', ') : 'none';
  const displayPrompt = ctx.prompt.length > 500 ? ctx.prompt.substring(0, 497) + '...' : ctx.prompt;

  return [
    {
      type: 'section',
      text: { type: 'mrkdwn', text: `> ${displayPrompt}` },
    },
    {
      type: 'section',
      text: { type: 'mrkdwn', text: triggeredCriteria.join('\n') },
    },
    {
      type: 'context',
      elements: [
        {
          type: 'mrkdwn',
          text: `*Category:* ${ctx.category} | *Type:* ${ctx.questionType} | *Topics:* ${topicsText} | _${analysis.summary}_`,
        },
      ],
    },
    { type: 'divider' },
  ];
}

export async function postBatchToSlack(results: AnalyzedPrompt[]): Promise<void> {
  const webhookUrl = process.env.SLACK_WEBHOOK_URL;
  if (!webhookUrl || results.length === 0) return;

  const blocks: Record<string, unknown>[] = [
    {
      type: 'header',
      text: {
        type: 'plain_text',
        text: `:speech_balloon: Docs Bot Digest — ${results.length} interesting prompt${results.length === 1 ? '' : 's'}`,
        emoji: true,
      },
    },
  ];

  for (const result of results) {
    blocks.push(...formatPromptBlock(result));
  }

  // Slack blocks limit is 50 — truncate if needed
  const message = { blocks: blocks.slice(0, 50) };

  const response = await fetch(webhookUrl, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(message),
  });

  if (!response.ok) {
    console.error(`[SlackReporter] Failed to post: ${response.status} ${response.statusText}`);
  }
}
