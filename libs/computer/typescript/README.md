# C/UA Computer TypeScript Library

The TypeScript library for C/UA Computer - a powerful computer control and automation library.

## Overview

This library is a TypeScript port of the Python computer library, providing the same functionality for controlling virtual machines and computer interfaces. It enables programmatic control of virtual machines through various providers and offers a consistent interface for interacting with the VM's operating system.

## Installation

```bash
npm install @cua/computer
# or
pnpm add @cua/computer
```

## Usage

```typescript
import { Computer } from '@cua/computer';

// Create a new computer instance
const computer = new CloudComputer({
  osType: OSType.LINUX,
  name: 's-linux-vm_id'
  apiKey: 'your-api-key'
});

// Start the computer
await computer.run();

// Get the computer interface for interaction
const interface = computer.interface;

// Take a screenshot
const screenshot = await interface.getScreenshot();

// Click at coordinates
await interface.click(500, 300);

// Type text
await interface.typeText('Hello, world!');

// Stop the computer
await computer.stop();
```

## Architecture

The library is organized into the following structure:

### Core Components

- **Computer Factory**: A factory object that creates appropriate computer instances
- **BaseComputer**: Abstract base class with shared functionality for all computer types
- **Types**: Type definitions for configuration options and shared interfaces

### Provider Implementations

- **LumeComputer**: Implementation for Lume API-based VMs (Unmaintained)
- **CloudComputer**: Implementation for cloud-based VMs

### Utility Functions

- **Lume API Utilities**: Functions for interacting with the Lume API (lumeApiGet, lumeApiRun, lumeApiStop, etc.) (Unmaintained)
- **Helper Functions**: Parsing utilities for display and memory strings

## Development

- Install dependencies:

```bash
pnpm install
```

- Run the unit tests:

```bash
pnpm test
```

- Build the library:

```bash
pnpm build
```

- Type checking:

```bash
pnpm typecheck
```

## Disclaimer

**WARNING:** Some parts of this library, particularly the provider implementations (like Lume), were created as test/example implementations and are not maintained or expected to work in production environments. They serve as references for how providers might be implemented but should not be used in production.

## License

[MIT](./LICENSE) License 2025 [C/UA](https://github.com/trycua)
