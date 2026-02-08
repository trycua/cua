"""Telemetry support for Agent class."""

import os
import platform
import sys
import time
from typing import Any, Dict, Optional

from core.telemetry import (
    flush,
    get_telemetry_client,
    increment,
    is_telemetry_enabled,
    record_event,
)

# System information used for telemetry
SYSTEM_INFO = {
    "os": sys.platform,
    "python_version": platform.python_version(),
}
