import { defineTool } from 'eve/tools';
import { z } from 'zod';

import { click, toModelContent } from '../lib/desktop.js';

export default defineTool({
  description:
    'Left-click an absolute desktop coordinate grounded in the latest get_desktop_state screenshot, then re-observe.',
  inputSchema: z.object({
    x: z.number().describe('Desktop x coordinate from the latest screenshot.'),
    y: z.number().describe('Desktop y coordinate from the latest screenshot.'),
  }),
  label: { start: ({ x, y }) => `Click at (${Math.round(x)}, ${Math.round(y)})` },
  async execute({ x, y }, ctx) {
    return await click(x, y, ctx.abortSignal);
  },
  toModelOutput: toModelContent,
});
