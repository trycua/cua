"""App registry implementation."""

from __future__ import annotations

from typing import (
    Any,
    Callable,
    Dict,
    List,
    Literal,
    Optional,
    Set,
    TypeVar,
    Union,
)

Platform = Literal["linux", "windows", "macos"]
ALL_PLATFORMS: Set[Platform] = {"linux", "windows", "macos"}

# Global registry
_registry: Dict[str, "App"] = {}


class AppMethod:
    """Descriptor for platform-specific app methods."""

    def __init__(
        self,
        method_type: str,
        platforms: Set[Platform],
        func: Callable,
    ):
        self.method_type = method_type
        self.platforms = platforms
        self.func = func

    async def __call__(self, session: Any, **kwargs) -> Any:
        return await self.func(session, **kwargs)


def _platform_decorator(method_type: str):
    """Factory for creating platform-specific method decorators."""

    def decorator(
        *platforms: Platform,
    ) -> Callable[[Callable], AppMethod]:
        """Decorator to register a platform-specific method.

        Args:
            *platforms: Platform names (linux, windows, macos).
                       If empty, applies to all platforms.
        """
        platform_set = set(platforms) if platforms else ALL_PLATFORMS

        def wrapper(func: Callable) -> AppMethod:
            return AppMethod(method_type, platform_set, func)

        return wrapper

    return decorator


# Public decorators
install = _platform_decorator("install")
launch = _platform_decorator("launch")
uninstall = _platform_decorator("uninstall")


class AppMeta(type):
    """Metaclass for App that auto-registers apps."""

    def __new__(mcs, name: str, bases: tuple, namespace: dict):
        cls = super().__new__(mcs, name, bases, namespace)

        # Don't register the base App class
        if name != "App" and hasattr(cls, "name") and cls.name:
            _registry[cls.name] = cls()

        return cls


class App(metaclass=AppMeta):
    """Base class for app definitions.

    Subclass this and define platform-specific methods using decorators:

        class MyApp(App):
            name = "myapp"
            description = "My application"

            @install("linux")
            async def install_linux(session, **kwargs):
                ...

            @install("windows")
            async def install_windows(session, **kwargs):
                ...

            @launch("linux", "windows")
            async def launch(session, **kwargs):
                ...
    """

    name: str = ""
    description: str = ""

    def get_method(
        self,
        method_type: str,
        platform: Platform,
    ) -> Optional[AppMethod]:
        """Get a method for the given type and platform."""
        for attr_name in dir(self):
            attr = getattr(self, attr_name)
            if isinstance(attr, AppMethod):
                if attr.method_type == method_type and platform in attr.platforms:
                    return attr
        return None

    def get_install(self, platform: Platform) -> Optional[AppMethod]:
        """Get the install method for a platform."""
        return self.get_method("install", platform)

    def get_launch(self, platform: Platform) -> Optional[AppMethod]:
        """Get the launch method for a platform."""
        return self.get_method("launch", platform)

    def get_uninstall(self, platform: Platform) -> Optional[AppMethod]:
        """Get the uninstall method for a platform."""
        return self.get_method("uninstall", platform)

    def supported_platforms(self, method_type: str = "install") -> Set[Platform]:
        """Get platforms supported for a method type."""
        platforms: Set[Platform] = set()
        for attr_name in dir(self):
            attr = getattr(self, attr_name)
            if isinstance(attr, AppMethod) and attr.method_type == method_type:
                platforms.update(attr.platforms)
        return platforms


def get_app(name: str) -> Optional[App]:
    """Get a registered app by name."""
    return _registry.get(name)


def list_apps() -> List[str]:
    """List all registered app names."""
    return list(_registry.keys())


class AppRegistry:
    """Registry access for DesktopSession integration.

    This class provides the interface used by DesktopSession to install/launch apps.
    """

    @staticmethod
    async def install_app(
        session: Any,
        app_name: str,
        *,
        with_shortcut: bool = True,
        **kwargs,
    ) -> None:
        """Install an app on the session's platform.

        Args:
            session: DesktopSession instance
            app_name: Name of the app to install
            with_shortcut: Whether to create desktop shortcut (default True)
            **kwargs: Additional app-specific arguments
        """
        app = get_app(app_name)
        if app is None:
            raise ValueError(
                f"Unknown app: {app_name}. Available: {list_apps()}"
            )

        # Get platform from session config
        platform = _get_platform(session)

        method = app.get_install(platform)
        if method is None:
            raise NotImplementedError(
                f"App '{app_name}' does not support install on {platform}. "
                f"Supported: {app.supported_platforms('install')}"
            )

        await method(session, with_shortcut=with_shortcut, **kwargs)

    @staticmethod
    async def launch_app(
        session: Any,
        app_name: str,
        **kwargs,
    ) -> None:
        """Launch an app on the session's platform.

        Args:
            session: DesktopSession instance
            app_name: Name of the app to launch
            **kwargs: App-specific launch arguments
        """
        app = get_app(app_name)
        if app is None:
            raise ValueError(
                f"Unknown app: {app_name}. Available: {list_apps()}"
            )

        platform = _get_platform(session)

        method = app.get_launch(platform)
        if method is None:
            raise NotImplementedError(
                f"App '{app_name}' does not support launch on {platform}. "
                f"Supported: {app.supported_platforms('launch')}"
            )

        await method(session, **kwargs)

    @staticmethod
    async def uninstall_app(
        session: Any,
        app_name: str,
        **kwargs,
    ) -> None:
        """Uninstall an app from the session's platform.

        Args:
            session: DesktopSession instance
            app_name: Name of the app to uninstall
            **kwargs: App-specific arguments
        """
        app = get_app(app_name)
        if app is None:
            raise ValueError(
                f"Unknown app: {app_name}. Available: {list_apps()}"
            )

        platform = _get_platform(session)

        method = app.get_uninstall(platform)
        if method is None:
            raise NotImplementedError(
                f"App '{app_name}' does not support uninstall on {platform}. "
                f"Supported: {app.supported_platforms('uninstall')}"
            )

        await method(session, **kwargs)


def _get_platform(session: Any) -> Platform:
    """Extract platform from session configuration."""
    # Try to get from session's config
    if hasattr(session, "_config") and session._config:
        os_type = session._config.get("os_type", "linux")
    elif hasattr(session, "os_type"):
        os_type = session.os_type
    else:
        os_type = "linux"

    # Normalize os_type to platform
    if os_type in ("linux",):
        return "linux"
    elif os_type in ("windows", "win11", "win10", "win7", "winxp", "win98"):
        return "windows"
    elif os_type in ("macos",):
        return "macos"
    else:
        return "linux"  # Default
