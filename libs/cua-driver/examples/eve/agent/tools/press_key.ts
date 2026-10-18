import { defineTool } from 'eve/tools';
import { z } from 'zod';

import { pressKey, toModelContent } from '../lib/desktop.js';

export default defineTool({
  description:
    'Press one named key, such as "return", "tab" or "escape", in the desktop session, then re-observe.',
  inputSchema: z.object({ key: z.string().min(1) }),
  label: { start: ({ key }) => `Press ${key}` },
  async execute({ key }, ctx) {
    return await pressKey(key, ctx.abortSignal);
  },
  toModelOutput: toModelContent,
});
