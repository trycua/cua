"""
Browser manager using Playwright for programmatic browser control.
This allows agents to control a browser that runs visibly on the XFCE desktop.
"""

import asyncio
import logging
import os
from typing import Any, Dict, Optional

try:
    from playwright.async_api import Browser, BrowserContext, Page, async_playwright
except ImportError:
    async_playwright = None
    Browser = None
    BrowserContext = None
    Page = None

logger = logging.getLogger(__name__)


class BrowserManager:
    """
    Manages a Playwright browser instance that runs visibly on the XFCE desktop.
    Uses persistent context to maintain cookies and sessions.
    """

    def __init__(self):
        """Initialize the BrowserManager."""
        self.playwright = None
        self.browser: Optional[Browser] = None
        self.context: Optional[BrowserContext] = None
        self.page: Optional[Page] = None
        self._initialized = False
        self._initialization_error: Optional[str] = None
        self._lock = asyncio.Lock()

    async def _ensure_initialized(self):
        """Ensure the browser is initialized."""
        # Check if browser was closed and needs reinitialization
        if self._initialized:
            try:
                # Check if context is still valid by trying to access it
                if self.context:
                    # Try to get pages - this will raise if context is closed
                    _ = self.context.pages
                    # If we get here, context is still alive
                    return
                else:
                    # Context was closed, need to reinitialize
                    self._initialized = False
                    logger.warning("Browser context was closed, will reinitialize...")
            except Exception as e:
                # Context is dead, need to reinitialize
                logger.warning(f"Browser context is dead ({e}), will reinitialize...")
                self._initialized = False
                self.context = None
                self.page = None
                # Clean up playwright if it exists
                if self.playwright:
                    try:
                        await self.playwright.stop()
                    except Exception:
                        pass
                    self.playwright = None

        async with self._lock:
            # Double-check after acquiring lock (another thread might have initialized it)
            if self._initialized:
                try:
                    if self.context:
                        _ = self.context.pages
                        return
                except Exception:
                    self._initialized = False
                    self.context = None
                    self.page = None
                    if self.playwright:
                        try:
                            await self.playwright.stop()
                        except Exception:
                            pass
                        self.playwright = None

            if async_playwright is None:
                raise RuntimeError(
                    "playwright is not installed. Please install it with: pip install playwright && playwright install --with-deps firefox"
                )

            try:
                # Get display from environment or default to :1
                display = os.environ.get("DISPLAY", ":1")
                logger.info(f"Initializing browser with DISPLAY={display}")

                # Start playwright
                self.playwright = await async_playwright().start()

                # Launch Firefox with persistent context (keeps cookies/sessions)
                # headless=False is CRITICAL so the visual agent can see it
                user_data_dir = os.path.join(os.path.expanduser("~"), ".playwright-firefox")
                os.makedirs(user_data_dir, exist_ok=True)

                # launch_persistent_context returns a BrowserContext, not a Browser
                # Note: Removed --kiosk mode so the desktop remains visible
                self.context = await self.playwright.firefox.launch_persistent_context(
                    user_data_dir=user_data_dir,
                    headless=False,  # CRITICAL: visible for visual agent
                    viewport={"width": 1024, "height": 768},
                    # Removed --kiosk to allow desktop visibility
                )

                # Add init script to make the browser less detectable
                await self.context.add_init_script(
                    """const defaultGetter = Object.getOwnPropertyDescriptor(
      Navigator.prototype,
      "webdriver"
    ).get;
    defaultGetter.apply(navigator);
    defaultGetter.toString();
    Object.defineProperty(Navigator.prototype, "webdriver", {
      set: undefined,
      enumerable: true,
      configurable: true,
      get: new Proxy(defaultGetter, {
        apply: (target, thisArg, args) => {
          Reflect.apply(target, thisArg, args);
          return false;
        },
      }),
    });
    const patchedGetter = Object.getOwnPropertyDescriptor(
      Navigator.prototype,
      "webdriver"
    ).get;
    patchedGetter.apply(navigator);
    patchedGetter.toString();"""
                )

                # Get the first page or create one
                pages = self.context.pages
                if pages:
                    self.page = pages[0]
                else:
                    self.page = await self.context.new_page()

                self._initialized = True
                logger.info("Browser initialized successfully")

            except Exception as e:
                logger.error(f"Failed to initialize browser: {e}")
                import traceback

                logger.error(traceback.format_exc())
                # Don't raise - return error in execute_command instead
                self._initialization_error = str(e)
                raise

    async def _execute_command_impl(self, cmd: str, params: Dict[str, Any]) -> Dict[str, Any]:
        """Internal implementation of command execution."""
        if cmd == "visit_url":
            url = params.get("url")
            if not url:
                return {"success": False, "error": "url parameter is required"}
            await self.page.goto(url, wait_until="domcontentloaded", timeout=30000)
            return {"success": True, "url": self.page.url}

        elif cmd == "click":
            x = params.get("x")
            y = params.get("y")
            if x is None or y is None:
                return {"success": False, "error": "x and y parameters are required"}
            await self.page.mouse.click(x, y)
            return {"success": True}

        elif cmd == "type":
            text = params.get("text")
            if text is None:
                return {"success": False, "error": "text parameter is required"}
            await self.page.keyboard.type(text)
            return {"success": True}

        elif cmd == "scroll":
            delta_x = params.get("delta_x", 0)
            delta_y = params.get("delta_y", 0)
            await self.page.mouse.wheel(delta_x, delta_y)
            return {"success": True}

        elif cmd == "web_search":
            query = params.get("query")
            if not query:
                return {"success": False, "error": "query parameter is required"}
            # Navigate to Google search
            search_url = f"https://www.google.com/search?q={query}"
            await self.page.goto(search_url, wait_until="domcontentloaded", timeout=30000)
            return {"success": True, "url": self.page.url}

        elif cmd == "screenshot":
            # Take a screenshot and return as base64
            import base64

            screenshot_bytes = await self.page.screenshot(type="png")
            screenshot_b64 = base64.b64encode(screenshot_bytes).decode("utf-8")
            return {"success": True, "screenshot": screenshot_b64}

        elif cmd == "get_current_url":
            # Get the current URL
            current_url = self.page.url
            return {"success": True, "url": current_url}

        elif cmd == "go_back":
            await self.page.go_back()
            return {"success": True, "url": self.page.url}

        elif cmd == "go_forward":
            await self.page.go_forward()
            return {"success": True, "url": self.page.url}

        else:
            return {"success": False, "error": f"Unknown command: {cmd}"}

    async def execute_command(self, cmd: str, params: Dict[str, Any]) -> Dict[str, Any]:
        """
        Execute a browser command with automatic recovery.

        Args:
            cmd: Command name (visit_url, click, type, scroll, web_search)
            params: Command parameters

        Returns:
            Result dictionary with success status and any data
        """
        max_retries = 2
        for attempt in range(max_retries):
            try:
                await self._ensure_initialized()
            except Exception as e:
                error_msg = getattr(self, "_initialization_error", None) or str(e)
                logger.error(f"Browser initialization failed: {error_msg}")
                return {
                    "success": False,
                    "error": f"Browser initialization failed: {error_msg}. "
                    f"Make sure Playwright and Firefox are installed, and DISPLAY is set correctly.",
                }

            # Check if page is still valid and get a new one if needed
            page_valid = False
            try:
                if self.page is not None and not self.page.is_closed():
                    # Try to access page.url to check if it's still valid
                    _ = self.page.url
                    page_valid = True
            except Exception as e:
                logger.warning(f"Page is invalid: {e}, will get a new page...")
                self.page = None

            # Get a valid page if we don't have one
            if not page_valid or self.page is None:
                try:
                    if self.context:
                        pages = self.context.pages
                        if pages:
                            # Find first non-closed page
                            for p in pages:
                                try:
                                    if not p.is_closed():
                                        self.page = p
                                        logger.info("Reusing existing open page")
                                        page_valid = True
                                        break
                                except Exception:
                                    continue

                        # If no valid page found, create a new one
                        if not page_valid:
                            self.page = await self.context.new_page()
                            logger.info("Created new page")
                except Exception as e:
                    logger.error(f"Failed to get new page: {e}, browser may be closed")
                    # Browser was closed - force reinitialization
                    self._initialized = False
                    self.context = None
                    self.page = None
                    if self.playwright:
                        try:
                            await self.playwright.stop()
                        except Exception:
                            pass
                        self.playwright = None

                    # If this isn't the last attempt, continue to retry
                    if attempt < max_retries - 1:
                        logger.info("Browser was closed, retrying with fresh initialization...")
                        continue
                    else:
                        return {
                            "success": False,
                            "error": f"Browser was closed and cannot be recovered: {e}",
                        }

            # Try to execute the command
            try:
                return await self._execute_command_impl(cmd, params)
            except Exception as e:
                error_str = str(e)
                logger.error(f"Error executing command {cmd}: {e}")

                # Check if this is a "browser/page/context closed" error
                if any(keyword in error_str.lower() for keyword in ["closed", "target", "context"]):
                    logger.warning(
                        f"Browser/page was closed during command execution (attempt {attempt + 1}/{max_retries})"
                    )

                    # Force reinitialization
                    self._initialized = False
                    self.context = None
                    self.page = None
                    if self.playwright:
                        try:
                            await self.playwright.stop()
                        except Exception:
                            pass
                        self.playwright = None

                    # If this isn't the last attempt, retry
                    if attempt < max_retries - 1:
                        logger.info("Retrying command after browser reinitialization...")
                        continue
                    else:
                        return {
                            "success": False,
                            "error": f"Command failed after {max_retries} attempts: {error_str}",
                        }
                else:
                    # Not a browser closed error, return immediately
                    import traceback

                    logger.error(traceback.format_exc())
                    return {"success": False, "error": error_str}

        # Should never reach here, but just in case
        return {"success": False, "error": "Command failed after all retries"}

    async def close(self):
        """Close the browser and cleanup resources."""
        async with self._lock:
            try:
                if self.context:
                    await self.context.close()
                    self.context = None
                if self.browser:
                    await self.browser.close()
                    self.browser = None

                if self.playwright:
                    await self.playwright.stop()
                    self.playwright = None

                self.page = None
                self._initialized = False
                logger.info("Browser closed successfully")
            except Exception as e:
                logger.error(f"Error closing browser: {e}")


# Global instance
_browser_manager: Optional[BrowserManager] = None


def get_browser_manager() -> BrowserManager:
    """Get or create the global BrowserManager instance."""
    global _browser_manager
    if _browser_manager is None:
        _browser_manager = BrowserManager()
    return _browser_manager
