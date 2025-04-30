#!/usr/bin/env bash

# Function to wait for SSH to become available
wait_for_ssh() {
    local host_ip=$1
    local user=$2
    local password=$3
    local retry_interval=${4:-5}   # Default retry interval is 5 seconds
    local max_retries=${5:-20}    # Default maximum retries is 20 (0 for infinite)

    echo "Waiting for SSH to become available on $host_ip..."

    local retry_count=0
    while true; do
        # Try to connect via SSH
        sshpass -p "$password" ssh -o StrictHostKeyChecking=no "$user@$host_ip" "exit"

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

        echo "SSH not ready. Retrying in $retry_interval seconds... (Attempt $retry_count)"
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

    echo "VNC password exported to VM: $vnc_password"

    data_folder_path="$VM_SHARED_FILES_PATH/$data_folder"
    echo "Data folder path in VM: $data_folder_path"

    # Read the script content and prepend the shebang
    script_content="#!/usr/bin/env bash\n"
    if [ -n "$data_folder" ]; then
        script_content+="export VNC_PASSWORD='$vnc_password'\n"
        script_content+="export DATA_FOLDER_PATH='$data_folder_path'\n"
    fi
    script_content+="$(<"$script_path")"

    # Use a here-document to send the script content
    sshpass -p "$password" ssh -o StrictHostKeyChecking=no "$user@$host" "bash -s" <<EOF
$script_content
EOF

    # Check the exit status of the sshpass command
    if [ $? -ne 0 ]; then
        echo "Failed to execute script on remote host $host."
        return 1
    fi
}

# Example usage
# output = execute_remote_script('192.168.1.100', 'username', 'password', '/path/to/script.sh')
# print(output)

extract_json_field() {
    local field_name=$1
    local input=$2
    local result
    result=$(echo "$input" | grep -oP '"'"$field_name"'"\s*:\s*"\K[^"]+')
    if [[ $? -ne 0 ]]; then
        echo ""
    else
        echo "$result"
    fi
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
