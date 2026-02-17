import Anthropic from '@anthropic-ai/sdk';

export interface PromptAnalysisResult {
  actionability: { score: number; rationale: string };
  tractionSignal: { score: number; rationale: string };
  useCaseInsight: { score: number; rationale: string };
  summary: string;
}

export interface PromptContext {
  prompt: string;
  category: string;
  questionType: string;
  topics: string[];
}

const SCORE_THRESHOLD = 7;

let anthropicClient: Anthropic | null = null;

function getClient(): Anthropic | null {
  if (!process.env.ANTHROPIC_API_KEY) return null;
  if (!anthropicClient) {
    anthropicClient = new Anthropic();
  }
  return anthropicClient;
}

function buildClassificationPrompt(ctx: PromptContext): string {
  return `Score this docs chatbot prompt on 3 criteria (1-10). Be selective — most should score low.

Criteria:
1. Actionability: Can we ship something to improve UX? (8+: clear docs gap or feature request, 1-4: normal question)
2. Traction Signal: Is someone actively building with the product? (8+: specific impl question or production issue, 1-4: curiosity)
3. Use Case Insight: Does this reveal how someone uses the product? (8+: names specific workflow/industry, 1-4: generic)

Prompt: "${ctx.prompt}"
Category: ${ctx.category} | Type: ${ctx.questionType} | Topics: ${ctx.topics.join(', ') || 'none'}

Respond ONLY with JSON, no markdown fences. Keep rationales to 5 words max.
{"actionability":{"score":N,"rationale":"..."},"traction_signal":{"score":N,"rationale":"..."},"use_case_insight":{"score":N,"rationale":"..."},"summary":"one short sentence"}`;
}

function parseAnalysisResponse(text: string): PromptAnalysisResult | null {
  try {
    const cleaned = text
      .replace(/```json?\s*/g, '')
      .replace(/```\s*/g, '')
      .trim();
    const parsed = JSON.parse(cleaned);

    const result: PromptAnalysisResult = {
      actionability: {
        score: Number(parsed.actionability?.score) || 0,
        rationale: String(parsed.actionability?.rationale || ''),
      },
      tractionSignal: {
        score: Number(parsed.traction_signal?.score) || 0,
        rationale: String(parsed.traction_signal?.rationale || ''),
      },
      useCaseInsight: {
        score: Number(parsed.use_case_insight?.score) || 0,
        rationale: String(parsed.use_case_insight?.rationale || ''),
      },
      summary: String(parsed.summary || ''),
    };

    const maxScore = Math.max(
      result.actionability.score,
      result.tractionSignal.score,
      result.useCaseInsight.score
    );

    if (maxScore < SCORE_THRESHOLD) return null;

    return result;
  } catch {
    return null;
  }
}

export async function analyzePromptForSlack(
  ctx: PromptContext
): Promise<PromptAnalysisResult | null> {
  const client = getClient();
  if (!client) return null;

  if (ctx.prompt.trim().length < 15) return null;

  const response = await client.messages.create({
    model: 'claude-haiku-4-5-20251001',
    max_tokens: 256,
    messages: [
      {
        role: 'user',
        content: buildClassificationPrompt(ctx),
      },
    ],
  });

  const text = response.content[0]?.type === 'text' ? response.content[0].text : '';

  return parseAnalysisResponse(text);
}
