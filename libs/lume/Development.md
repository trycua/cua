# Development Guide

This guide will help you set up your development environment and understand the process for contributing code to lume.

## Environment Setup

Lume development requires:

- Swift 6 or higher
- Xcode 15 or higher
- macOS Sequoia 15.2 or higher
- (Optional) VS Code with Swift extension

If you're working on Lume in the context of the Cua monorepo, we recommend using the dedicated VS Code workspace configuration:

```bash
# Open VS Code workspace from the root of the monorepo
code .vscode/lume.code-workspace
```

This workspace is preconfigured with Swift language support, build tasks, and debug configurations.

## Setting Up the Repository Locally

1. **Fork the Repository**: Create your own fork of lume
2. **Clone the Repository**:
   ```bash
   git clone https://github.com/trycua/lume.git
   cd lume
   ```
3. **Install Dependencies**:
   ```bash
   swift package resolve
   ```
4. **Build the Project**:
   ```bash
   swift build
   ```

## Development Workflow

1. Create a new branch for your changes
2. Make your changes
3. Run the tests: `swift test`
4. Build and test your changes locally
5. Commit your changes with clear commit messages

## Submitting Pull Requests

1. Push your changes to your fork
2. Open a Pull Request with:
   - A clear title and description
   - Reference to any related issues
   - Screenshots or logs if relevant
3. Respond to any feedback from maintainers
