"""Constants for the MCP client."""

# Default host and port for MCP server connections
DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 7681  # Standard MCP port

# Connection timeout in seconds
DEFAULT_CONNECTION_TIMEOUT = 5

# Retry settings
MAX_RETRIES = 3
RETRY_DELAY = 1.5  # seconds

# Logging settings
LOG_FORMAT = "%(asctime)s - %(name)s - %(levelname)s - %(message)s" 