import { defineTool } from 'eve/tools';
import { z } from 'zod';

import { toModelContent, typeText } from '../lib/desktop.js';

export default defineTool({
  description: 'Type text into the focused desktop control through Cua Driver, then re-observe.',
  inputSchema: z.object({ text: z.string().min(1) }),
  // Show the length, not the text: typed values can be private.
  label: { start: ({ text }) => `Type ${text.length} characters` },
  async execute({ text }, ctx) {
    return await typeText(text, ctx.abortSignal);
  },
  toModelOutput: toModelContent,
});
