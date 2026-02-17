import { NextRequest, NextResponse } from 'next/server';
import { fetchRecentPrompts } from '@/lib/posthog-query';
import { analyzeBatchForSlack } from '@/lib/prompt-analyzer';
import { postBatchToSlack } from '@/lib/slack-reporter';

export async function GET(req: NextRequest) {
  // Verify the request is from Vercel Cron
  const authHeader = req.headers.get('authorization');
  if (process.env.CRON_SECRET && authHeader !== `Bearer ${process.env.CRON_SECRET}`) {
    return NextResponse.json({ error: 'Unauthorized' }, { status: 401 });
  }

  try {
    const prompts = await fetchRecentPrompts(1);

    if (prompts.length === 0) {
      return NextResponse.json({ status: 'ok', message: 'No prompts in the last hour' });
    }

    const interesting = await analyzeBatchForSlack(prompts);

    if (interesting.length === 0) {
      return NextResponse.json({
        status: 'ok',
        message: `Analyzed ${prompts.length} prompts, none met threshold`,
      });
    }

    await postBatchToSlack(interesting);

    return NextResponse.json({
      status: 'ok',
      message: `Posted ${interesting.length}/${prompts.length} interesting prompts to Slack`,
    });
  } catch (err) {
    console.error('[PromptDigest] Cron failed:', err);
    return NextResponse.json(
      { error: 'Internal error', message: err instanceof Error ? err.message : String(err) },
      { status: 500 }
    );
  }
}
