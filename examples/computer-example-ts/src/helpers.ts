import type { Computer } from '@trycua/computer';
import type OpenAI from 'openai';

export async function executeAction(
  computer: Computer,
  action: OpenAI.Responses.ResponseComputerToolCall['action']
) {
  switch (action.type) {
    case 'click': {
      const { x, y, button } = action;
      console.log(`Executing click at (${x}, ${y}) with button '${button}'.`);
      await computer.interface.moveCursor(x, y);
      if (button === 'right') await computer.interface.rightClick();
      else await computer.interface.leftClick();
      break;
    }
    case 'type':
      {
        const { text } = action;
        console.log(`Typing text: ${text}`);
        await computer.interface.typeText(text);
      }
      break;
    case 'scroll': {
      const { x: locX, y: locY, scroll_x, scroll_y } = action;
      console.log(
        `Scrolling at (${locX}, ${locY}) with offsets (scroll_x=${scroll_x}, scroll_y=${scroll_y}).`
      );
      await computer.interface.moveCursor(locX, locY);
      await computer.interface.scroll(scroll_x, scroll_y);
      break;
    }
    case 'keypress': {
      const { keys } = action;
      for (const key of keys) {
        console.log(`Pressing key: ${key}.`);
        // Map common key names to CUA equivalents
        if (key.toLowerCase() === 'enter') {
          await computer.interface.pressKey('return');
        } else if (key.toLowerCase() === 'space') {
          await computer.interface.pressKey('space');
        } else {
          await computer.interface.pressKey(key);
        }
      }
      break;
    }
    case 'wait': {
      console.log(`Waiting for 3 seconds.`);
      await new Promise((resolve) => setTimeout(resolve, 3 * 1000));
      break;
    }
    case 'screenshot': {
      console.log('Taking screenshot.');
      // This is handled automatically in the main loop, but we can take an extra one if requested
      const screenshot = await computer.interface.screenshot();
      return screenshot;
    }
    default:
      console.log(`Unrecognized action: ${action.type}`);
      break;
  }
}
