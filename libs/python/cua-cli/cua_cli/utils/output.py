"""Output formatting utilities for CUA CLI."""

import json
import sys
from typing import Any

from rich.console import Console
from rich.table import Table

console = Console()
error_console = Console(stderr=True)


def print_table(
    data: list[dict[str, Any]],
    columns: list[tuple[str, str]] | None = None,
    title: str | None = None,
) -> None:
    """Print data as a formatted table.

    Args:
        data: List of dictionaries to display
        columns: List of (key, header) tuples. If None, uses all keys from first item.
        title: Optional table title
    """
    if not data:
        console.print("[dim]No data to display[/dim]")
        return

    table = Table(title=title, show_header=True, header_style="bold")

    if columns is None:
        columns = [(k, k.upper()) for k in data[0].keys()]

    for _, header in columns:
        table.add_column(header)

    for item in data:
        row = [str(item.get(key, "")) for key, _ in columns]
        table.add_row(*row)

    console.print(table)


def print_json(data: Any) -> None:
    """Print data as formatted JSON."""
    console.print_json(json.dumps(data, indent=2, default=str))


def print_error(message: str) -> None:
    """Print an error message to stderr."""
    error_console.print(f"[red]Error:[/red] {message}")


def print_success(message: str) -> None:
    """Print a success message."""
    console.print(f"[green]{message}[/green]")


def print_warning(message: str) -> None:
    """Print a warning message."""
    console.print(f"[yellow]Warning:[/yellow] {message}")


def print_info(message: str) -> None:
    """Print an info message."""
    console.print(f"[blue]{message}[/blue]")
