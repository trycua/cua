# Contributing to Cua

We deeply appreciate your interest in contributing to Cua! Whether you're reporting bugs, suggesting enhancements, improving docs, or submitting pull requests, your contributions help improve the project for everyone.

## Reporting Bugs

If you've encountered a bug in the project, we encourage you to report it. Please follow these steps:

1. **Check the Issue Tracker**: Before submitting a new bug report, please check our issue tracker to see if the bug has already been reported.
2. **Create a New Issue**: If the bug hasn't been reported, create a new issue with:
   - A clear title and detailed description
   - Steps to reproduce the issue
   - Expected vs actual behavior
   - Your environment (macOS version, lume version)
   - Any relevant logs or error messages
3. **Label Your Issue**: Label your issue as a `bug` to help maintainers identify it quickly.

## Suggesting Enhancements

We're always looking for suggestions to make lume better. If you have an idea:

1. **Check Existing Issues**: See if someone else has already suggested something similar.
2. **Create a New Issue**: If your enhancement is new, create an issue describing:
   - The problem your enhancement solves
   - How your enhancement would work
   - Any potential implementation details
   - Why this enhancement would benefit lume users

## Code Formatting

We follow strict code formatting guidelines to ensure consistency across the codebase. Before submitting any code:

1. **Review Our Format Guide**: Please review our [Code Formatting Standards](Development.md#code-formatting-standards) section in the Getting Started guide.
2. **Configure Your IDE**: We recommend using the workspace settings provided in `.vscode/` for automatic formatting.
3. **Run Formatting Tools**: Always run the formatting tools before submitting a PR:
   ```bash
   # For Python code
   uv run black .
   uv run isort .
   uv run ruff check --fix .
   ```
4. **Validate Your Code**: Ensure your code passes all checks:
   ```bash
   uv run mypy .
   ```
5. Every time you try to commit code, a pre-commit hook will automatically run the formatting and validation tools. If any issues are found, the commit will be blocked until they are resolved. Please make sure to address any issues reported by the pre-commit hook before attempting to commit again. Once all issues are resolved, you can proceed with your commit.

## Documentation

Documentation improvements are always welcome. You can:

- Fix typos or unclear explanations
- Add examples and use cases
- Improve API documentation
- Add tutorials or guides

For detailed instructions on setting up your development environment and submitting code contributions, please see our [Developer-Guide](Development.md).

Feel free to join our [Discord community](https://discord.com/invite/mVnXXpdE85) to discuss ideas or get help with your contributions.
