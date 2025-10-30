#!/bin/bash

# Only run if we're in the cua project
if [[ ! -f "$CLAUDE_PROJECT_DIR/pyproject.toml" ]]; then
  echo "Skipping style checks - not in cua project directory"
  exit 0
fi

cd "$CLAUDE_PROJECT_DIR"

echo "Running isort check..."
uv run isort --check-only . || echo "⚠️  isort found issues"

echo "Running black check..."
uv run black --check . || echo "⚠️  black found issues"

echo "Running ruff check..."
uv run ruff check . || echo "⚠️  ruff found issues"

echo "✓ Style checks complete"
