"""Data models for the app catalog."""

from __future__ import annotations

from datetime import datetime
from typing import Literal, Optional

from pydantic import BaseModel, Field


class InstallMethod(BaseModel):
    """Per-OS installation metadata and script references."""

    os: Literal["linux", "windows", "macos", "android"]
    package_manager: Optional[str] = None
    package_id: Optional[str] = None
    download_url: Optional[str] = None
    download_hash: Optional[str] = None
    install_script: str  # relative path: "linux/install.sh"
    launch_script: str  # relative path: "linux/launch.sh"
    test_file: str  # relative path: "linux/test_install.py"
    install_dir: Optional[str] = None
    binary_path: Optional[str] = None
    dependencies: list[str] = Field(default_factory=list)
    post_install_commands: list[str] = Field(default_factory=list)
    verified: bool = False
    evidence_screenshot: Optional[str] = None
    last_verified: Optional[datetime] = None


class AppEntry(BaseModel):
    """A single software application in the catalog."""

    # Identity
    id: str  # slug: "blender"
    name: str  # "Blender"
    description: str = ""
    website: str = ""
    icon_url: Optional[str] = None

    # Classification
    categories: list[str] = Field(default_factory=list)
    tags: list[str] = Field(default_factory=list)
    soc_groups: list[str] = Field(default_factory=list)

    # Availability
    os_support: list[Literal["linux", "windows", "macos", "android"]] = Field(default_factory=list)
    # True if a CUA agent would need to pay (credit card / subscription) to
    # fully exercise the app's core UI. Free tiers, trials, OSS = False.
    requires_payment: bool = False
    # True if the software is Free & Open Source (MIT, Apache, GPL, etc.)
    foss: bool = False
    # Public source repo URL if available (e.g. "https://github.com/godotengine/godot")
    gh_repo: Optional[str] = None
    self_hostable: bool = False
    requires_hardware: bool = False

    # Install info per OS
    install_methods: dict[str, InstallMethod] = Field(default_factory=dict)

    # GDP attribution
    estimated_gdp_usd: Optional[float] = None

    # Replica mode: for clones of proprietary services with /gym endpoints
    app_type: Optional[Literal["native", "webapp", "library", "replica"]] = None
    replica_source: Optional[str] = None  # e.g. "venmo-clone"
    replica_gym_url: Optional[str] = None  # e.g. "http://localhost:5000/gym"
    real_app_name: Optional[str] = None  # e.g. "Venmo"
    real_app_docs: list[str] = Field(default_factory=list)
    upload_files: list[dict] = Field(default_factory=list)  # [{local_dir, sandbox_path}]

    # Provenance
    discovered_via: str = "unknown"
    verified: bool = False
