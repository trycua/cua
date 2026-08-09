"""Workload identity federation token commands."""

import argparse
import sys

from cua_cli.auth.github_wif import GitHubWifError, request_github_wif_token
from cua_cli.utils.async_utils import run_async
from cua_cli.utils.output import print_error


def register_parser(subparsers: argparse._SubParsersAction) -> None:
    parser = subparsers.add_parser(
        "wif-token",
        help="Request a workload identity federation token",
        description="Request a provider-issued workload identity token",
    )
    providers = parser.add_subparsers(
        dest="wif_token_provider", help="Workload identity provider"
    )
    providers.add_parser(
        "github",
        help="Request a GitHub Actions OIDC token for Fleets",
        description="Print a GitHub Actions OIDC token for Fleets",
    )


def execute(args: argparse.Namespace) -> int:
    if getattr(args, "wif_token_provider", None) == "github":
        return cmd_github(args)
    print_error("Usage: cua wif-token github")
    return 1


def cmd_github(_args: argparse.Namespace) -> int:
    try:
        token = run_async(request_github_wif_token())
    except (GitHubWifError, OSError) as error:
        print_error(str(error))
        return 1
    sys.stdout.write(f"{token}\n")
    return 0
