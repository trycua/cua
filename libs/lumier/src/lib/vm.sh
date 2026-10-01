#!/usr/bin/env bash

# Initialize global flags
export PULL_IN_PROGRESS=0

start_vm() {
    # Determine storage path for VM
    STORAGE_PATH="$HOST_STORAGE_PATH"
    if [ -z "$STORAGE_PATH" ]; then
        STORAGE_PATH="storage_${VM_NAME}"
    fi

    # Check if VM exists and its status using JSON format - quietly
    VM_INFO=$(lume_get "$VM_NAME" "$STORAGE_PATH" "json" "${LUMIER_DEBUG:-0}")

    # Check if VM not found error
    if [[ $VM_INFO == *"Virtual machine not found"* ]]; then
        IMAGE_NAME="${VERSION##*/}"
        # Parse registry and organization from VERSION
        REGISTRY=$(echo $VERSION | cut -d'/' -f1)
        ORGANIZATION=$(echo $VERSION | cut -d'/' -f2)
        
        echo "Pulling VM image $IMAGE_NAME..."
        lume_pull "$IMAGE_NAME" "$VM_NAME" "$STORAGE_PATH" "$REGISTRY" "$ORGANIZATION"
    else
        # Parse the JSON status - check if it contains "status" : "running"
        if [[ $VM_INFO == *'"status" : "running"'* ]]; then
            lume_stop "$VM_NAME" "$STORAGE_PATH"
        fi
    fi

    # Format memory size for display purposes
    MEMORY_DISPLAY="$RAM_SIZE"
    if [[ ! "$RAM_SIZE" == *"GB"* && ! "$RAM_SIZE" == *"MB"* ]]; then
        MEMORY_DISPLAY="${RAM_SIZE}MB"
    fi
    
    # Set VM parameters using the wrapper function
    if [[ "$LUMIER_DEBUG" == "1" ]]; then
        echo "Updating VM settings: cpu=$CPU_CORES memory=$MEMORY_DISPLAY display=$DISPLAY"
    fi
    lume_set "$VM_NAME" "$STORAGE_PATH" "$CPU_CORES" "$RAM_SIZE" "$DISPLAY"

    # Fetch VM configuration - quietly (don't display to console)
    CONFIG_JSON=$(lume_get "$VM_NAME" "$STORAGE_PATH" "json" "${LUMIER_DEBUG:-0}")
    
    # Setup shared directory args if necessary
    SHARED_DIR_ARGS=""
    if [ -d "/shared" ]; then
        if [ -n "$HOST_SHARED_PATH" ]; then
            SHARED_DIR_ARGS="--shared-dir=$HOST_SHARED_PATH"
        else
            echo "Warning: /shared volume exists but HOST_SHARED_PATH is not set. Cannot mount volume."
        fi
    fi

    # Run VM with VNC and shared directory using curl
    lume_run $SHARED_DIR_ARGS --storage "$STORAGE_PATH" "$VM_NAME" &
    # lume run "$VM_NAME" --storage "$STORAGE_PATH" --no-display

    # sleep 10000000

    # Wait for VM to be running and VNC URL to be available
    vm_ip=""
    vnc_url=""
    max_attempts=30
    attempt=0
    
    while [ $attempt -lt $max_attempts ]; do
            # Get VM info as JSON using the API function - pass debug flag
        VM_INFO=$(lume_get "$VM_NAME" "$STORAGE_PATH" "json" "${LUMIER_DEBUG:-0}")
        
        # Extract status, IP address, and VNC URL using the helper function
        vm_status=$(extract_json_field "status" "$VM_INFO")
        vm_ip=$(extract_json_field "ipAddress" "$VM_INFO")
        vnc_url=$(extract_json_field "vncUrl" "$VM_INFO")

        # Check if VM status is 'running' and we have IP and VNC URL
        if [ "$vm_status" = "running" ] && [ -n "$vm_ip" ] && [ -n "$vnc_url" ]; then
            break
        fi
        
        sleep 2
        attempt=$((attempt + 1))
    done
    
    if [ -z "$vm_ip" ] || [ -z "$vnc_url" ]; then
        echo "Timed out waiting for VM to start or VNC URL to become available."
        lume_stop "$VM_NAME" "$STORAGE_PATH" > /dev/null 2>&1
        # lume stop "$VM_NAME" --storage "$STORAGE_PATH" > /dev/null 2>&1
        exit 1
    fi

    # Parse VNC URL to extract password and port
    VNC_PASSWORD=$(echo "$vnc_url" | sed -n 's/.*:\(.*\)@.*/\1/p')
    VNC_PORT=$(echo "$vnc_url" | sed -n 's/.*:\([0-9]\+\)$/\1/p')
    
    # Wait for SSH to become available
    wait_for_ssh "$vm_ip" "$HOST_USER" "$HOST_PASSWORD" 5 20

    # Export VNC variables for entry.sh to use
    export VNC_PORT
    export VNC_PASSWORD
    
    # Execute on-logon.sh if present
    on_logon_script="/run/lifecycle/on-logon.sh"
    
    # Only show detailed logs in debug mode
    if [ "${LUMIER_DEBUG:-0}" == "1" ]; then
        echo "Running on-logon.sh hook script on VM..."
    fi
    
    # Check if script exists
    if [ ! -f "$on_logon_script" ]; then
        echo "Warning: on-logon.sh hook script not found at $on_logon_script"
    else
        # Execute the remote script
        execute_remote_script "$vm_ip" "$HOST_USER" "$HOST_PASSWORD" "$on_logon_script" "$VNC_PASSWORD" "$HOST_SHARED_PATH"
    fi
}

# Get VM information using curl
lume_get() {
    local vm_name="$1"
    local storage="$2"
    local format="${3:-json}"
    local debug="${4:-false}"
    
    local api_host="${LUME_API_HOST:-host.docker.internal}"
    local api_port="${LUME_API_PORT:-7777}"
    
    # URL encode the storage path for the query parameter
    # Replace special characters with their URL encoded equivalents
    local encoded_storage=$(echo "$storage" | sed 's/\//%2F/g' | sed 's/ /%20/g' | sed 's/:/%3A/g')
    
    # Construct API URL with encoded storage parameter
    local api_url="http://${api_host}:${api_port}/lume/vms/${vm_name}?storage=${encoded_storage}"
    
    # Construct the full curl command
    local curl_cmd="curl --connect-timeout 6000 --max-time 5000 -s '$api_url'"
    
    # Print debug info
    if [[ "$debug" == "true" || "$LUMIER_DEBUG" == "1" ]]; then
        echo "[DEBUG] Calling API: $api_url"
        echo "[DEBUG] Full curl command: $curl_cmd"
    fi
    
    # Log curl commands only when in debug mode
    if [[ "$debug" == "true" || "$LUMIER_DEBUG" == "1" ]]; then
        echo "[$(date -u '+%Y-%m-%dT%H:%M:%SZ')] DEBUG: Executing curl request: $api_url" >&2
    fi
    
    # Make the API call
    local response=$(curl --connect-timeout 6000 \
      --max-time 5000 \
      -s \
      "$api_url")
    
    # Print the response if debugging is enabled
    if [[ "$debug" == "true" || "${LUMIER_DEBUG:-0}" == "1" ]]; then
        echo "[DEBUG] API Response:"
        echo "$response" | jq '.' 2>/dev/null || echo "$response"
    fi
    
    # Output the response so callers can capture it
    echo "$response"
}

# Set VM properties using curl
lume_set() {
    local vm_name="$1"
    local storage="$2"
    local cpu="${3:-4}"
    local memory="${4:-8192}"
    local display="${5:-1024x768}"
    
    local api_host="${LUME_API_HOST:-host.docker.internal}"
    local api_port="${LUME_API_PORT:-7777}"
    
    # Handle memory format for the API
    if [[ "$memory" == *"GB"* ]]; then
        # Already in GB format, keep as is
        :  # No-op
    elif [[ "$memory" =~ ^[0-9]+$ ]]; then
        # If memory is a simple number, assume MB and convert to GB
        memory="$(awk "BEGIN { printf \"%.1f\", $memory/1024 }")GB"
    fi
    
    # Only show memory formatting debug in debug mode
    if [[ "$LUMIER_DEBUG" == "1" ]]; then
        echo "[DEBUG] Formatted memory value: $memory"
    fi
    
    # Store response to conditionally show based on debug mode
    local response=$(curl --connect-timeout 6000 \
      --max-time 5000 \
      -s \
      -X PATCH \
      -H "Content-Type: application/json" \
      -d "{
        \"cpu\": $cpu,
        \"memory\": \"$memory\",
        \"display\": \"$display\",
        \"storage\": \"$storage\"
      }" \
      "http://${api_host}:${api_port}/lume/vms/${vm_name}")
      
    # Only show response in debug mode
    if [[ "${LUMIER_DEBUG:-0}" == "1" ]]; then
        echo "$response"
    fi
}

stop_vm() {
    local in_cleanup=${1:-false} # Optional first argument to indicate if called from cleanup trap
    echo "Stopping VM '$VM_NAME'..."
    STORAGE_PATH="$HOST_STORAGE_PATH"
    
    # Only show storage path in debug mode
    if [[ "$LUMIER_DEBUG" == "1" ]]; then
        echo "STORAGE_PATH: $STORAGE_PATH"
    fi
    
    VM_INFO=$(lume_get "$VM_NAME" "$STORAGE_PATH" "json" "${LUMIER_DEBUG:-0}")
    vm_status=$(extract_json_field "status" "$VM_INFO")

    if [ "$vm_status" == "running" ]; then
        lume_stop "$VM_NAME" "$STORAGE_PATH"
    elif [ "$vm_status" == "stopped" ]; then
        echo "VM '$VM_NAME' is already stopped."
    elif [ "$in_cleanup" = true ]; then
        # If we are in the cleanup trap and status is unknown or VM not found, 
        # still attempt a stop just in case.
        echo "VM status is unknown ('$vm_status') or VM not found during cleanup. Attempting stop anyway."
        lume_stop "$VM_NAME" "$STORAGE_PATH"
        sleep 5
        echo "VM '$VM_NAME' stop command issued as a precaution."
    else
        echo "VM status is unknown ('$vm_status') or VM not found. Not attempting stop."
    fi
}

is_vm_running() {
    # Check VM status using the API function
    local vm_info
    vm_info=$(lume_get "$VM_NAME" "$HOST_STORAGE_PATH")
    if [[ $vm_info == *'"status" : "running"'* ]]; then
        return 0 # Running
    else
        return 1 # Not running or doesn't exist
    fi
    # lume ls | grep -q "$VM_NAME" # Old CLI check
}

# Stop VM with storage location specified using curl
lume_stop() {
    local vm_name="$1"
    local storage="$2"
    
    local api_host="${LUME_API_HOST:-host.docker.internal}"
    local api_port="${LUME_API_PORT:-7777}"
    
    # Only log in debug mode
    if [[ "$LUMIER_DEBUG" == "1" ]]; then
        echo "Stopping VM $vm_name..."
    fi
    
    # Execute command and capture response
    local response
    if [[ "${LUMIER_DEBUG:-0}" == "1" ]]; then
        # Show output in debug mode
        response=$(curl --connect-timeout 6000 \
          --max-time 5000 \
          -X POST \
          -H "Content-Type: application/json" \
          -d '{"storage":"'$storage'"}' \
          "http://${api_host}:${api_port}/lume/vms/${vm_name}/stop")
        echo "$response"
    else
        # Run silently in normal mode
        response=$(curl --connect-timeout 6000 \
          --max-time 5000 \
          -s \
          -X POST \
          -H "Content-Type: application/json" \
          -d '{"storage":"'$storage'"}' \
          "http://${api_host}:${api_port}/lume/vms/${vm_name}/stop")
    fi
}

# Pull a VM image using curl
lume_pull() {
    local image="$1"      # Image name with tag
    local vm_name="$2"    # Name for the new VM
    local storage="$3"    # Storage location
    local registry="${4:-ghcr.io}"  # Registry, default is ghcr.io
    local organization="${5:-trycua}" # Organization, default is trycua
    
    local api_host="${LUME_API_HOST:-host.docker.internal}"
    local api_port="${LUME_API_PORT:-7777}"
    
    # Mark that pull is in progress for interrupt handling
    export PULL_IN_PROGRESS=1
    
    # Only log full details in debug mode
    if [[ "$LUMIER_DEBUG" == "1" ]]; then
        echo "Pulling image $image from $registry/$organization..."
    else
        echo "Pulling image $image..."
    fi
    
    # Inform users how to check pull progress
    echo "You can check the pull progress using: lume logs -f"
    
    # Pull image via API and capture response
    local response
    if [[ "${LUMIER_DEBUG:-0}" == "1" ]]; then
        # Show full response in debug mode - no timeout limits
        response=$(curl \
          -X POST \
          -H "Content-Type: application/json" \
          -d "{
            \"image\": \"$image\",
            \"name\": \"$vm_name\",
            \"registry\": \"$registry\",
            \"organization\": \"$organization\",
            \"storage\": \"$storage\"
          }" \
          "http://${api_host}:${api_port}/lume/pull")
        echo "$response"
    else
        # Run silently in normal mode - no timeout limits
        response=$(curl \
          -s \
          -X POST \
          -H "Content-Type: application/json" \
          -d "{
            \"image\": \"$image\",
            \"name\": \"$vm_name\",
            \"registry\": \"$registry\",
            \"organization\": \"$organization\",
            \"storage\": \"$storage\"
          }" \
          "http://${api_host}:${api_port}/lume/pull")
    fi
    
    # Unset pull in progress flag
    export PULL_IN_PROGRESS=0
}


# Run VM with VNC client started and shared directory using curl
lume_run() {
    # Parse args
    local shared_dir=""
    local storage=""
    local vm_name="lume_vm"
    local no_display=true
    while [[ $# -gt 0 ]]; do
        case $1 in
            --shared-dir=*)
                shared_dir="${1#*=}"
                shift
                ;;
            --storage)
                storage="$2"
                shift 2
                ;;
            --no-display)
                no_display=true
                shift
                ;;
            *)
                # Assume last arg is VM name if not an option
                vm_name="$1"
                shift
                ;;
        esac
    done
    
    local api_host="${LUME_API_HOST:-host.docker.internal}"
    local api_port="${LUME_API_PORT:-7777}"

    # Only log in debug mode
    if [[ "$LUMIER_DEBUG" == "1" ]]; then
        echo "Running VM $vm_name..."
    fi
    
    # Build the JSON body dynamically based on what's provided
    local json_body="{\"noDisplay\": true"
    
    # Only include shared directories if shared_dir is provided
    if [[ -n "$shared_dir" ]]; then
        json_body+=", \"sharedDirectories\": [{\"hostPath\": \"$shared_dir\", \"readOnly\": false}]"
    fi
    
    # Only include storage if it's provided
    if [[ -n "$storage" ]]; then
        json_body+=", \"storage\": \"$storage\""
    fi
    
    # Add recovery mode (always false)
    json_body+=", \"recoveryMode\": false}"

    # Execute the command and store the response
    local response
    if [[ "${LUMIER_DEBUG:-0}" == "1" ]]; then
        # Show response in debug mode
        response=$(curl --connect-timeout 6000 \
          --max-time 5000 \
          -X POST \
          -H 'Content-Type: application/json' \
          -d "$json_body" \
          http://${api_host}:${api_port}/lume/vms/$vm_name/run)
        echo "$response"
    else
        # Run silently in normal mode
        response=$(curl --connect-timeout 6000 \
          --max-time 5000 \
          -s \
          -X POST \
          -H 'Content-Type: application/json' \
          -d "$json_body" \
          http://${api_host}:${api_port}/lume/vms/$vm_name/run)
    fi
}

# Delete a VM using curl
lume_delete() {
    local vm_name="$1"
    local storage="$2"
    
    local api_host="${LUME_API_HOST:-host.docker.internal}"
    local api_port="${LUME_API_PORT:-7777}"
    
    # URL encode the storage path for the query parameter
    # Replace special characters with their URL encoded equivalents
    local encoded_storage=$(echo "$storage" | sed 's/\//%2F/g' | sed 's/ /%20/g' | sed 's/:/%3A/g')
    
    # Construct API URL with encoded storage parameter
    local api_url="http://${api_host}:${api_port}/lume/vms/${vm_name}?storage=${encoded_storage}"
    
    # Only log in debug mode
    if [[ "$LUMIER_DEBUG" == "1" ]]; then
        echo "Deleting VM $vm_name from storage $storage..."
    fi
    
    # Execute command and capture response
    local response
    if [[ "${LUMIER_DEBUG:-0}" == "1" ]]; then
        # Show output in debug mode
        response=$(curl --connect-timeout 6000 \
          --max-time 5000 \
          -X DELETE \
          "$api_url")
        echo "$response"
    else
        # Run silently in normal mode
        response=$(curl --connect-timeout 6000 \
          --max-time 5000 \
          -s \
          -X DELETE \
          "$api_url")
    fi
}