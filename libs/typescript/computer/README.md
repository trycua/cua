<div align="center">
<h1>
  <div class="image-wrapper" style="display: inline-block;">
    <picture>
      <source media="(prefers-color-scheme: dark)" alt="logo" height="150" srcset="https://raw.githubusercontent.com/trycua/cua/main/img/logo_white.png" style="display: block; margin: auto;">
      <source media="(prefers-color-scheme: light)" alt="logo" height="150" srcset="https://raw.githubusercontent.com/trycua/cua/main/img/logo_black.png" style="display: block; margin: auto;">
      <img alt="Shows my svg">
    </picture>
  </div>

[![TypeScript](https://img.shields.io/badge/TypeScript-333333?logo=typescript&logoColor=white&labelColor=333333)](#)
[![macOS](https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=F0F0F0)](#)
[![Discord](https://img.shields.io/badge/Discord-%235865F2.svg?&logo=discord&logoColor=white)](https://discord.com/invite/mVnXXpdE85)
[![NPM](https://img.shields.io/npm/v/@trycua/computer?color=333333)](https://www.npmjs.com/package/@trycua/computer)

</h1>
</div>

**@trycua/computer** is a Computer-Use Interface (CUI) framework powering Cua for interacting with local macOS and Linux sandboxes, Playwright-compatible, and pluggable with any AI agent systems (Cua, Langchain, CrewAI, AutoGen). Computer relies on [Lume](https://github.com/trycua/lume) for creating and managing sandbox environments.

### Get started with Computer

<div align="center">
    <img src="https://raw.githubusercontent.com/trycua/cua/main/img/computer.png"/>
</div>

```typescript
import { Computer, OSType } from '@trycua/computer';

// Create a new computer instance
const computer = new Computer({
  osType: OSType.LINUX,
  name: 's-linux-vm_id',
  apiKey: 'your-api-key',
});

// Start the computer
await computer.run();

// Get the computer interface for interaction
const computerInterface = computer.interface;

// Take a screenshot
const screenshot = await computerInterface.getScreenshot();
// In a Node.js environment, you might save it like this:
// import * as fs from 'fs';
// fs.writeFileSync('screenshot.png', Buffer.from(screenshot));

// Click at coordinates
await computerInterface.click(500, 300);

// Type text
await computerInterface.typeText('Hello, world!');

// Stop the computer
await computer.stop();
```

## Install

To install the Computer-Use Interface (CUI):

```bash
npm install @trycua/computer
# or
pnpm add @trycua/computer
```

The `@trycua/computer` package provides the TypeScript library for interacting with computer interfaces.

## Run

Refer to this example for a step-by-step guide on how to use the Computer-Use Interface (CUI):

- [Computer-Use Interface (CUI)](https://github.com/trycua/cua/tree/main/examples/computer-example-ts)

## Docs

- [Computer SDK](https://cua.ai/docs/cua/reference/computer-sdk)

## License

[MIT](./LICENSE) License 2025 [Cua](https://github.com/trycua)
