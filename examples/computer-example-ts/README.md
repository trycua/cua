# cua-cloud-openai Example

This example demonstrates how to control a Cua Cloud Sandbox using the OpenAI `computer-use-preview` model and the `@trycua/computer` TypeScript library.

## Overview

- Connects to a Cua Cloud Sandbox via the `@trycua/computer` library
- Sends screenshots and instructions to OpenAI's computer-use model
- Executes AI-generated actions (clicks, typing, etc.) inside the sandbox
- Designed for Linux sandboxes, but can be adapted for other OS types

## Getting Started

1. **Install dependencies:**

   ```bash
   npm install
   ```

2. **Set up environment variables:**
   Create a `.env` file with the following variables:
   - `OPENAI_API_KEY` — your OpenAI API key
   - `CUA_API_KEY` — your Cua Cloud API key
   - `CUA_CONTAINER_NAME` — the name of your provisioned sandbox

3. **Run the example:**

   ```bash
   npx tsx src/index.ts
   ```

## Files

- `src/index.ts` — Main example script
- `src/helpers.ts` — Helper for executing actions on the container

## Further Reading

For a step-by-step tutorial and more detailed explanation, see the accompanying blog post:

➡️ [Controlling a Cua Cloud Sandbox with JavaScript](https://placeholder-url-to-blog-post.com)

_(This link will be updated once the article is published.)_

---

If you have questions or issues, please open an issue or contact the maintainers.
