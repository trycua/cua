#!/usr/bin/env bash

start_vm() {
    # Determine storage path for VM
    STORAGE_PATH="$HOST_STORAGE_PATH"
    if [ -z "$STORAGE_PATH" ]; then
        STORAGE_PATH="storage_${VM_NAME}"
    fi

    # Check if VM exists and its status using JSON format
    VM_INFO=$(lume get "$VM_NAME" --storage "$STORAGE_PATH" -f json 2>&1)

    # Check if VM not found error
    if [[ $VM_INFO == *"Virtual machine not found"* ]]; then
        IMAGE_NAME="${VERSION##*/}"
        lume pull "$IMAGE_NAME" "$VM_NAME" --storage "$STORAGE_PATH"
    else
        # Parse the JSON status - check if it contains "status" : "running"
        if [[ $VM_INFO == *'"status" : "running"'* ]]; then
            lume_stop "$VM_NAME" "$STORAGE_PATH"
            # lume stop "$VM_NAME" --storage "$STORAGE_PATH"
        fi
    fi

    # Set VM parameters
    lume set "$VM_NAME" --cpu "$CPU_CORES" --memory "${RAM_SIZE}MB" --display "$DISPLAY" --storage "$STORAGE_PATH"

    # Fetch VM configuration
    CONFIG_JSON=$(lume get "$VM_NAME" --storage "$STORAGE_PATH" -f json)
    
    # Setup data directory args if necessary
    SHARED_DIR_ARGS=""
    if [ -d "/data" ]; then
        if [ -n "$HOST_DATA_PATH" ]; then
            SHARED_DIR_ARGS="--shared-dir=$HOST_DATA_PATH"
        else
            echo "Warning: /data volume exists but HOST_DATA_PATH is not set. Cannot mount volume."
        fi
    fi

    # Run VM with VNC and shared directory using curl
    lume_run $SHARED_DIR_ARGS --storage "$STORAGE_PATH" "$VM_NAME" &
    # lume run "$VM_NAME" --storage "$STORAGE_PATH" --no-display

    # Wait for VM to be running and VNC URL to be available
    vm_ip=""
    vnc_url=""
    max_attempts=30
    attempt=0
    
    while [ $attempt -lt $max_attempts ]; do
        # Get VM info as JSON
        VM_INFO=$(lume get "$VM_NAME" --storage "$STORAGE_PATH" -f json 2>/dev/null)
        
        # Check if VM has status 'running'
        if [[ $VM_INFO == *'"status" : "running"'* ]]; then
            # Extract IP address using the existing function from utils.sh
            vm_ip=$(extract_json_field "ipAddress" "$VM_INFO")
            # Extract VNC URL using the existing function from utils.sh
            vnc_url=$(extract_json_field "vncUrl" "$VM_INFO")
            
            # If we have both IP and VNC URL, break the loop
            if [ -n "$vm_ip" ] && [ -n "$vnc_url" ]; then
                break
            fi
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
    if [ -f "$on_logon_script" ]; then
        execute_remote_script "$vm_ip" "$HOST_USER" "$HOST_PASSWORD" "$on_logon_script" "$VNC_PASSWORD" "$DATA_FOLDER"
    fi

    # The VM is still running because we never killed lume run.
    # If you want to stop the VM at some point, you can kill $LUME_PID or use lume_stop.
}

stop_vm() {
    echo "Stopping VM '$VM_NAME'..."
    STORAGE_PATH="$HOST_STORAGE_PATH"
    if [ -z "$STORAGE_PATH" ]; then
        STORAGE_PATH="storage_${VM_NAME}"
    fi
    # Check if the VM exists and is running (use lume get for speed)
    VM_INFO=$(lume get "$VM_NAME" --storage "$STORAGE_PATH" -f json 2>/dev/null)
    if [[ -z "$VM_INFO" || $VM_INFO == *"Virtual machine not found"* ]]; then
        echo "VM '$VM_NAME' does not exist."
    elif [[ $VM_INFO == *'"status" : "running"'* ]]; then
        lume_stop "$VM_NAME" "$STORAGE_PATH"
        echo "VM '$VM_NAME' was running and is now stopped."
    elif [[ $VM_INFO == *'"status" : "stopped"'* ]]; then
        echo "VM '$VM_NAME' is already stopped."
    else
        echo "Unknown VM status for '$VM_NAME'."
    fi
}

is_vm_running() {
    lume ls | grep -q "$VM_NAME"
}

# Stop VM with storage location specified using curl
lume_stop() {
    local vm_name="$1"
    local storage="$2"
    curl --connect-timeout 6000 \
      --max-time 5000 \
      -X POST \
      -H "Content-Type: application/json" \
      -d '{"storage":"'$storage'"}' \
      "http://host.docker.internal:3000/lume/vms/${vm_name}/stop"
}

# Run VM with VNC client started and shared directory using curl
lume_run() {
    # Parse args
    local shared_dir=""
    local storage="ssd"
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
    
    # Default to ~/Projects if not provided
    if [[ -z "$shared_dir" ]]; then
        shared_dir="~/Projects"
    fi
    
    local json_body="{\"noDisplay\": true, \"sharedDirectories\": [{\"hostPath\": \"$shared_dir\", \"readOnly\": false}], \"storage\": \"$storage\", \"recoveryMode\": false}"
    local curl_cmd="curl --connect-timeout 6000 \
      --max-time 5000 \
      -X POST \
      -H 'Content-Type: application/json' \
      -d '$json_body' \
      http://host.docker.internal:3000/lume/vms/$vm_name/run"
    echo "[lume_run] Running:"
    echo "$curl_cmd"
    eval "$curl_cmd"
}