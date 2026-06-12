import { createAmazonBedrock } from '@ai-sdk/amazon-bedrock';
import { generateText } from 'ai';
import { awsCredentialsProvider } from '@vercel/functions/oidc';

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

export interface AnalyzedPrompt {
  analysis: PromptAnalysisResult;
  ctx: PromptContext;
}

const SCORE_THRESHOLD = 7;

// Default AWS role ARN for Vercel OIDC
const AWS_ROLE_ARN =
  process.env.AWS_ROLE_ARN || 'arn:aws:iam::296062593712:role/vercel-bedrock-role';

// Cache for AWS credentials (tokens are cached for up to 45 min by Vercel)
let cachedCredentials: {
  accessKeyId: string;
  secretAccessKey: string;
  sessionToken?: string;
} | null = null;
let credentialExpiry: number = 0;

function createBedrockProvider() {
  return createAmazonBedrock({
    region: 'us-east-1',
    credentialProvider: async () => {
      // Check for static credentials first (for local dev)
      if (process.env.AWS_ACCESS_KEY_ID && process.env.AWS_SECRET_ACCESS_KEY) {
        return {
          accessKeyId: process.env.AWS_ACCESS_KEY_ID,
          secretAccessKey: process.env.AWS_SECRET_ACCESS_KEY,
          sessionToken: process.env.AWS_SESSION_TOKEN,
        };
      }

      // Return cached credentials if still valid (with 5 min buffer)
      if (cachedCredentials && Date.now() < credentialExpiry - 5 * 60 * 1000) {
        return cachedCredentials;
      }

      // Use Vercel's awsCredentialsProvider which handles OIDC token retrieval server-side
      const credProvider = awsCredentialsProvider({
        roleArn: AWS_ROLE_ARN,
      });

      const creds = await credProvider();
      cachedCredentials = {
        accessKeyId: creds.accessKeyId,
        secretAccessKey: creds.secretAccessKey,
        sessionToken: creds.sessionToken,
      };
      // Cache for 40 minutes (Vercel caches token for 45 min max)
      credentialExpiry = Date.now() + 40 * 60 * 1000;

      return cachedCredentials;
    },
  });
}

function buildBatchPrompt(prompts: PromptContext[]): string {
  const promptList = prompts
    .map(
      (p, i) =>
        `[${i}] "${p.prompt}" (category: ${p.category}, type: ${p.questionType}, topics: ${p.topics.join(', ') || 'none'})`
    )
    .join('\n');

  return `Score each docs chatbot prompt on 3 criteria (1-10). Be selective — most should score low.

Criteria:
1. Actionability: Can we ship something to improve UX? (8+: clear docs gap or feature request, 1-4: normal question)
2. Traction Signal: Is someone actively building with the product? (8+: specific impl question or production issue, 1-4: curiosity)
3. Use Case Insight: Does this reveal how someone uses the product? (8+: names specific workflow/industry, 1-4: generic)

Prompts:
${promptList}

Respond ONLY with a JSON array, no markdown fences. Keep rationales to 5 words max. Only include prompts scoring 7+ on at least one criterion.
[{"index":N,"actionability":{"score":N,"rationale":"..."},"traction_signal":{"score":N,"rationale":"..."},"use_case_insight":{"score":N,"rationale":"..."},"summary":"short sentence"}]
If no prompts are interesting, respond with: []`;
}

function parseBatchResponse(text: string, prompts: PromptContext[]): AnalyzedPrompt[] {
  try {
    const cleaned = text
      .replace(/```json?\s*/g, '')
      .replace(/```\s*/g, '')
      .trim();
    const parsed = JSON.parse(cleaned);

    if (!Array.isArray(parsed)) return [];

    const results: AnalyzedPrompt[] = [];

    for (const item of parsed) {
      const index = Number(item.index);
      if (isNaN(index) || index < 0 || index >= prompts.length) continue;

      const analysis: PromptAnalysisResult = {
        actionability: {
          score: Number(item.actionability?.score) || 0,
          rationale: String(item.actionability?.rationale || ''),
        },
        tractionSignal: {
          score: Number(item.traction_signal?.score) || 0,
          rationale: String(item.traction_signal?.rationale || ''),
        },
        useCaseInsight: {
          score: Number(item.use_case_insight?.score) || 0,
          rationale: String(item.use_case_insight?.rationale || ''),
        },
        summary: String(item.summary || ''),
      };

      const maxScore = Math.max(
        analysis.actionability.score,
        analysis.tractionSignal.score,
        analysis.useCaseInsight.score
      );

      if (maxScore >= SCORE_THRESHOLD) {
        results.push({ analysis, ctx: prompts[index] });
      }
    }

    return results;
  } catch {
    return [];
  }
}

export async function analyzeBatchForSlack(prompts: PromptContext[]): Promise<AnalyzedPrompt[]> {
  // Filter out trivial prompts
  const filtered = prompts.filter((p) => p.prompt.trim().length >= 15);
  if (filtered.length === 0) return [];

  const bedrock = createBedrockProvider();

  const { text } = await generateText({
    model: bedrock('us.anthropic.claude-haiku-4-5-20251001-v1:0'),
    maxOutputTokens: 4096,
    prompt: buildBatchPrompt(filtered),
  });

  return parseBatchResponse(text, filtered);
}
