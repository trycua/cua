#!/usr/bin/env bash

# Source constants if running in container context
if [ -f "/run/config/constants.sh" ]; then
  source "/run/config/constants.sh"
fi

# Define server address with fallback
SERVER="${TUNNEL_HOST:-host.docker.internal}:${TUNNEL_PORT:-8080}"

# Extract the base name of the command and arguments
command=$(basename "$0")
subcommand="$1"
shift
args="$@"

command="$command $subcommand $args"

# Concatenate command and any stdin data
full_data="$command"
if [ ! -t 0 ]; then
  stdin_data=$(cat)
  if [ -n "$stdin_data" ]; then
    # Format full_data to include stdin data
    full_data="$full_data << 'EOF'
    $stdin_data
EOF"
  fi
fi

# Trim leading/trailing whitespace and newlines
full_data=$(echo -e "$full_data" | sed 's/^[ \t\n]*//;s/[ \t\n]*$//')

# Log command if debug is enabled
if [ "${LUMIER_DEBUG:-0}" -eq 1 ]; then
  echo "Executing lume command: $full_data" >&2
  echo "Sending to: $SERVER" >&2
fi

# Use curl with -N to disable output buffering and -s for silent mode
curl -N -s -X POST \
  -H "Content-Type: application/octet-stream" \
  --data-binary @- \
  "http://$SERVER" <<< "$full_data" 