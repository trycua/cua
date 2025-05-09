# Telemetry in CUA

This document explains how telemetry works in CUA libraries and how you can control it.

CUA tracks anonymized usage and error report statistics; we ascribe to Posthog's approach as detailed [here](https://posthog.com/blog/open-source-telemetry-ethical). If you would like to opt out of sending anonymized info, you can set `telemetry_enabled` to false.

## What telemetry data we collect

CUA libraries collect minimal anonymous usage data to help improve our software. The telemetry data we collect is specifically limited to:

- Basic system information:
  - Operating system (e.g., 'darwin', 'win32', 'linux')
  - Python version (e.g., '3.11.0')
- Module initialization events:
  - When a module (like 'computer' or 'agent') is imported
  - Version of the module being used

We do NOT collect:
- Personal information
- Contents of files
- Specific text being typed
- Actual screenshots or screen contents
- User-specific identifiers
- API keys
- File contents
- Application data or content
- User interactions with the computer
- Information about files being accessed

## Controlling Telemetry

We are committed to transparency and user control over telemetry. There are two ways to control telemetry:

## 1. Environment Variable (Global Control)

Telemetry is enabled by default. To disable telemetry, set the `CUA_TELEMETRY_ENABLED` environment variable to a falsy value (`0`, `false`, `no`, or `off`):

```bash
# Disable telemetry before running your script
export CUA_TELEMETRY_ENABLED=false

# Or as part of the command
CUA_TELEMETRY_ENABLED=1 python your_script.py

```
Or from Python:
```python
import os
os.environ["CUA_TELEMETRY_ENABLED"] = "false"
```

## 2. Instance-Level Control

You can control telemetry for specific CUA instances by setting `telemetry_enabled` when creating them:

```python
# Disable telemetry for a specific Computer instance
computer = Computer(telemetry_enabled=False)

# Enable telemetry for a specific Agent instance
agent = ComputerAgent(telemetry_enabled=True)
```

You can check if telemetry is enabled for an instance:

```python
print(computer.telemetry_enabled)  # Will print True or False
```

Note that telemetry settings must be configured during initialization and cannot be changed after the object is created.

## Transparency

We believe in being transparent about the data we collect. If you have any questions about our telemetry practices, please open an issue on our GitHub repository.