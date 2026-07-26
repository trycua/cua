"""Authentication commands for Cua CLI."""

import argparse
import sys
import webbrowser
from datetime import UTC, datetime

from cua_cli.auth.oidc import OidcClient, OidcError
from cua_cli.auth.store import (
    CredentialStorageError,
    clear_credentials,
    load_credentials,
    save_credentials,
)
from cua_cli.utils.async_utils import run_async
from cua_cli.utils.output import print_error, print_info, print_success


def register_parser(subparsers: argparse._SubParsersAction) -> None:
    """Register authentication commands."""
    auth_parser = subparsers.add_parser(
        "auth",
        help="Authentication commands",
        description="Manage run.cua.ai authentication",
    )
    auth_subparsers = auth_parser.add_subparsers(dest="auth_command", help="Authentication command")

    login_parser = auth_subparsers.add_parser(
        "login", help="Log in with device authorization", description="Authenticate with run.cua.ai"
    )
    login_parser.add_argument(
        "--no-browser",
        action="store_true",
        help="Do not try to open the verification URL automatically",
    )
    auth_subparsers.add_parser("logout", help="Revoke and remove local credentials")
    auth_subparsers.add_parser("status", help="Show local authentication status")


def execute(args: argparse.Namespace) -> int:
    """Execute an authentication subcommand."""
    command = getattr(args, "auth_command", None)
    if command == "login":
        return cmd_login(args)
    if command == "logout":
        return cmd_logout(args)
    if command == "status":
        return cmd_status(args)
    print_error("Usage: cua auth <login|logout|status>")
    return 1


def cmd_login(args: argparse.Namespace) -> int:
    """Start OAuth device authorization and store the resulting tokens."""
    client = OidcClient()
    try:
        discovery = run_async(client.discover())
        device_code = run_async(client.request_device_code(discovery))
    except (OidcError, OSError) as error:
        print_error(f"Could not start device authorization: {error}")
        return 1

    verification_url = device_code.verification_uri_complete or device_code.verification_uri
    print_info(f"Open this URL in any browser: {verification_url}")
    print_info(f"Enter this code if prompted: {device_code.user_code}")
    if not args.no_browser and sys.stdin.isatty() and sys.stdout.isatty():
        try:
            if webbrowser.open(verification_url):
                print_info("Opened the verification URL in your browser.")
        except webbrowser.Error:
            print_info("Could not open a browser. Use the URL shown above.")
    elif not args.no_browser:
        print_info("No interactive terminal detected; use the URL shown above in a browser.")

    try:
        credentials = run_async(client.poll_for_tokens(discovery, device_code))
        save_credentials(credentials)
    except (CredentialStorageError, OidcError, OSError) as error:
        print_error(f"Login failed: {error}")
        return 1

    print_success("Logged in to run.cua.ai.")
    return 0


def cmd_logout(_args: argparse.Namespace) -> int:
    """Revoke the refresh token when possible, then remove local credentials."""
    try:
        credentials = load_credentials()
    except CredentialStorageError as error:
        print_error(str(error))
        return 1
    if credentials is None:
        print_info("Not logged in.")
        return 0

    try:
        client = OidcClient()

        async def revoke_remote() -> bool:
            return await client.revoke(await client.discover(), credentials)

        revoked = run_async(revoke_remote())
        if not revoked:
            print_info(
                "Remote token revocation was unavailable; removing local credentials anyway."
            )
    except (OidcError, OSError):
        print_info(
            "Could not reach run.cua.ai to revoke the token; removing local credentials anyway."
        )

    try:
        clear_credentials()
    except CredentialStorageError as error:
        print_error(str(error))
        return 1
    print_success("Logged out.")
    return 0


def cmd_status(_args: argparse.Namespace) -> int:
    """Show local session availability without exposing token material."""
    try:
        credentials = load_credentials()
    except CredentialStorageError as error:
        print_error(str(error))
        return 1
    if credentials is None:
        print_info("Not logged in. Run 'cua auth login'.")
        return 1

    now = datetime.now(UTC)
    if credentials.expires_at <= now:
        print_info(
            "Logged in; access token expired and will refresh on the next run.cua.ai request."
        )
    else:
        print_success(
            f"Logged in to run.cua.ai. Access token expires {credentials.expires_at.isoformat()}."
        )
    return 0
