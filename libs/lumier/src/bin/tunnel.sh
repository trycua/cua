#!/usr/bin/env bash

# Exit on errors, undefined variables, and propagate errors in pipes
set -euo pipefail

# Source constants if available
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ -f "${SCRIPT_DIR}/../config/constants.sh" ]; then
  source "${SCRIPT_DIR}/../config/constants.sh"
fi

# Handle errors and cleanup
cleanup() {
  local exit_code=$?
  # Clean up any temporary files if they exist
  [ -n "${temp_file:-}" ] && [ -f "$temp_file" ] && rm "$temp_file"
  [ -n "${fifo:-}" ] && [ -p "$fifo" ] && rm "$fifo"
  exit $exit_code
}
trap cleanup EXIT INT TERM

log_debug() {
  if [ "${LUMIER_DEBUG:-0}" -eq 1 ]; then
    echo "[DEBUG] $*" >&2
  fi
}

send_error_response() {
  local status_code=$1
  local message=$2
  echo "HTTP/1.1 $status_code"
  echo "Content-Type: text/plain"
  echo ""
  echo "$message"
  exit 1
}

# Read the HTTP request line
read -r request_line
log_debug "Request: $request_line"

# Read headers and look for Content-Length
content_length=0
while IFS= read -r header; do
    [[ $header == $'\r' ]] && break  # End of headers
    log_debug "Header: $header"
    if [[ "$header" =~ ^Content-Length:\ ([0-9]+) ]]; then
        content_length="${BASH_REMATCH[1]}"
    fi
done

# Read the body using the content length
command=""
if [ "$content_length" -gt 0 ]; then
    command=$(dd bs=1 count="$content_length" 2>/dev/null)
    log_debug "Received command: $command"
fi

# Determine the executable and arguments based on the command
if [[ "$command" == lume* ]]; then
    executable="$(which lume || echo "/usr/local/bin/lume")"
    command_args="${command#lume}"  # Remove 'lume' from the command
elif [[ "$command" == sshpass* ]]; then
    executable="$(which sshpass || echo "/usr/local/bin/sshpass")"
    command_args="${command#sshpass}"
else
    send_error_response "400 Bad Request" "Unsupported command: $command"
fi

# Check if executable exists
if [ ! -x "$executable" ]; then
    send_error_response "500 Internal Server Error" "Executable not found or not executable: $executable"
fi

# Create a temporary file to store the command
temp_file=$(mktemp)
echo "$executable $command_args" > "$temp_file"
chmod +x "$temp_file"

# Create a FIFO (named pipe) for capturing output
fifo=$(mktemp -u)
mkfifo "$fifo"

# Execute the command and pipe its output through awk to ensure line-buffering
{
    log_debug "Executing: $executable $command_args"
    "$temp_file" 2>&1 | awk '{ print; fflush() }' > "$fifo"
} &

# Stream the output from the FIFO as an HTTP response
{
    echo -e "HTTP/1.1 200 OK\r"
    echo -e "Content-Type: text/plain\r"
    echo -e "\r"
    cat "$fifo"
} 