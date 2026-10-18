import { defineTool } from 'eve/tools';
import { z } from 'zod';

import { observe, toModelContent } from '../lib/desktop.js';

export default defineTool({
  description:
    'Capture the whole primary desktop through Cua Driver. Use before every action and to verify its result.',
  inputSchema: z.object({}),
  label: { start: () => 'Look at the desktop' },
  async execute(_input, ctx) {
    return await observe(ctx.abortSignal);
  },
  toModelOutput: toModelContent,
});
