# Cua TypeScript Libraries

This repository contains TypeScript implementations of the Cua libraries:

- `@trycua/core`: Core functionality including telemetry and logging
- `@trycua/fleet`: Browser and Node.js SDK for sandbox templates, pools, claims, and services
- `@trycua/computer`: Legacy computer interaction and control compatibility SDK

## Project Structure

```text
libs/typescript/
├── computer/       # Computer-control compatibility package
├── core/           # Core functionality package
├── fleet/          # Fleet sandbox lifecycle package
├── package.json    # Root package configuration
└── pnpm-workspace.yaml  # Workspace configuration
```

## Prerequisites

- [Node.js](https://nodejs.org/) (v20 or later)
- [pnpm](https://pnpm.io/) (v10 or later)

## Setup and Installation

1. Install dependencies for all packages:

```bash
pnpm install
```

1. Build all packages:

```bash
pnpm build:all
```

## Development Workflow

### Building Packages

Build all packages in the correct dependency order:

```bash
pnpm build:all
```

Build specific packages:

```bash
# Build core package
pnpm --filter @trycua/core build

# Build Fleet package
pnpm --filter @trycua/fleet build

# Build computer compatibility package
pnpm --filter @trycua/computer build
```

### Running Tests

Run tests for all packages:

```bash
pnpm test:all
```

Run tests for specific packages:

```bash
# Test core package
pnpm --filter @trycua/core test

# Test computer package
pnpm --filter @trycua/computer test
```

### Linting

Lint all packages:

```bash
pnpm lint:all
```

Fix linting issues:

```bash
pnpm lint:fix:all
```

## Package Details

### @trycua/core

Core functionality for Cua libraries including:

- Telemetry with PostHog integration
- Common utilities and types

### @trycua/computer

Compatibility SDK for existing computer-control integrations. Use `@trycua/fleet`
for new sandbox lifecycle code instead of managing Fleet resources here.

- Legacy VM provider system (Cloud)
- Interface system for OS-specific interactions
- Screenshot, keyboard, and mouse control
- Command execution

## Publishing

Prepare packages for publishing:

```bash
pnpm -r build
```

Publish packages:

```bash
pnpm -r publish
```
