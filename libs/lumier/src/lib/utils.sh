#!/usr/bin/env bash

# Function to wait for SSH to become available
wait_for_ssh() {
    local host_ip=$1
    local user=$2
    local password=$3
    local retry_interval=${4:-5}   # Default retry interval is 5 seconds
    local max_retries=${5:-20}    # Default maximum retries is 20 (0 for infinite)

    # Only show waiting message in debug mode
    if [ "${LUMIER_DEBUG:-0}" == "1" ]; then
        echo "Waiting for SSH to become available on $host_ip..."
    fi

    local retry_count=0
    while true; do
        # Try to connect via SSH
        # Add -q for completely silent operation, redirect stderr to /dev/null
        sshpass -p "$password" ssh -q -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "$user@$host_ip" "exit" 2>/dev/null

        # Check the exit status of the SSH command
        if [ $? -eq 0 ]; then
            echo "SSH is ready on $host_ip!"
            return 0
        fi

        # Increment retry count
        ((retry_count++))
        
        # Exit if maximum retries are reached
        if [ $max_retries -ne 0 ] && [ $retry_count -ge $max_retries ]; then
            echo "Maximum retries reached. SSH is not available."
            return 1
        fi

        # Only show retry messages in debug mode
        if [ "${LUMIER_DEBUG:-0}" == "1" ]; then
            echo "SSH not ready. Retrying in $retry_interval seconds... (Attempt $retry_count)"
        fi
        sleep $retry_interval
    done
}

# Function to execute a script on a remote server using sshpass
execute_remote_script() {
    local host="$1"
    local user="$2"
    local password="$3"
    local script_path="$4"
    local vnc_password="$5"
    local data_folder="$6"

    # Check if all required arguments are provided
    if [ -z "$host" ] || [ -z "$user" ] || [ -z "$password" ] || [ -z "$script_path" ] || [ -z "$vnc_password" ]; then
        echo "Usage: execute_remote_script <host> <user> <password> <script_path> <vnc_password> [data_folder]"
        return 1
    fi

    # Only show VNC info in debug mode
    if [ "${LUMIER_DEBUG:-0}" == "1" ]; then
        echo "VNC password exported to VM: $vnc_password"
    fi

    # Set the shared folder path for the VM
    if [ -n "$data_folder" ]; then
        # VM always sees shared folders at this path, regardless of container path
        shared_folder_path="/Volumes/My Shared Files"
        
        # Only show path in debug mode
        if [ "${LUMIER_DEBUG:-0}" == "1" ]; then
            echo "Data folder path in VM: $shared_folder_path"
        fi
    else
        shared_folder_path=""
    fi

    # Read the script content and prepend the shebang
    script_content="#!/usr/bin/env bash\n"
    # Always export VNC_PASSWORD
    script_content+="export VNC_PASSWORD='$vnc_password'\n"
    # Export SHARED_FOLDER_PATH only if we have a data folder path
    if [ -n "$shared_folder_path" ]; then
        script_content+="export SHARED_FOLDER_PATH='$shared_folder_path'\n"
    fi
    # Pass debug setting to the VM
    script_content+="export VNC_DEBUG='${LUMIER_DEBUG:-0}'\n"
    
    # Add debug messages only if debug mode is enabled
    if [[ "${LUMIER_DEBUG:-0}" == "1" ]]; then
        script_content+="echo \"[DEBUG] Starting on-logon script execution...\"\n"
    fi
    
    # Add the original script content
    script_content+="$(<"$script_path")"
    
    # Add debug messages only if debug mode is enabled
    if [[ "${LUMIER_DEBUG:-0}" == "1" ]]; then
        script_content+="\necho \"[DEBUG] Finished executing on-logon script.\"\n"
    fi
    
    # Print debug info only when debug mode is enabled
    if [[ "${LUMIER_DEBUG:-0}" == "1" ]]; then
        echo "[DEBUG] Executing remote script with content length: $(echo -n "$script_content" | wc -c) bytes"
        echo "[DEBUG] Script path: $script_path"
    fi
    
    # Use a here-document to send the script content
    # We'll capture both stdout and stderr when debug is enabled
    if [[ "${LUMIER_DEBUG:-0}" == "1" ]]; then
        echo "[DEBUG] Connecting to $user@$host to execute script..."
        sshpass -p "$password" ssh -q -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "$user@$host" "bash -s -- '$vnc_password' '$data_folder'" 2>&1 <<EOF
$script_content
EOF
    else
        # Otherwise run quietly
        sshpass -p "$password" ssh -q -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o LogLevel=ERROR "$user@$host" "bash -s -- '$vnc_password' '$data_folder'" 2>/dev/null <<EOF
$script_content
EOF
    fi

    # Print completion message only in debug mode
    if [[ "${LUMIER_DEBUG:-0}" == "1" ]]; then
        echo "[DEBUG] Script execution completed."
    fi

    # Check the exit status of the sshpass command
    if [ $? -ne 0 ]; then
        echo "Failed to execute script on remote host $host."
        return 1
    fi
}

extract_json_field() {
    local field_name=$1
    local input=$2
    local result=""
    
    # First attempt with jq if available (most reliable JSON parsing)
    if command -v jq &> /dev/null; then
        # Use jq for reliable JSON parsing
        result=$(echo "$input" | jq -r ".$field_name // empty" 2>/dev/null)
        if [[ -n "$result" ]]; then
            echo "$result"
            return 0
        fi
    fi
    
    # Fallback to grep-based approach with improvements
    # First try for quoted string values
    result=$(echo "$input" | tr -d '\n' | grep -o "\"$field_name\"\s*:\s*\"[^\"]*\"" | sed -E 's/.*":\s*"(.*)"$/\1/')
    if [[ -n "$result" ]]; then
        echo "$result"
        return 0
    fi
    
    # Try for non-quoted values (numbers, true, false, null)
    result=$(echo "$input" | tr -d '\n' | grep -o "\"$field_name\"\s*:\s*[^,}]*" | sed -E 's/.*":\s*(.*)$/\1/')
    if [[ -n "$result" ]]; then
        echo "$result"
        return 0
    fi
    
    # Return empty string if field not found
    echo ""
}

extract_json_field_from_file() {
    local field_name=$1
    local json_file=$2
    local json_text
    json_text=$(<"$json_file")
    extract_json_field "$field_name" "$json_text"
}

extract_json_field_from_text() {
    local field_name=$1
    local json_text=$2
    extract_json_field "$field_name" "$json_text"
}
