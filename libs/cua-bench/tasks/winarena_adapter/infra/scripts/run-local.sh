#!/bin/bash

# Exit immediately if a command exits with a non-zero status
set -e

source ./shared.sh

# Default parameters
mode="azure" # Default to azure if no argument is provided
prepare_image=false
skip_build=false
interactive=false
connect=false
use_kvm=true
ram_size=8G
cpu_cores=8
mount_vm_storage=true
mount_client=true
mount_server=true
container_name="winarena"
browser_port=8006
rdp_port=3390
start_client=true
agent="navi"
model="gpt-4-vision-preview"
som_origin="oss"
a11y_backend="uia"
gpu_enabled=false

# Parse the command line arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --container-name)
            container_name="$2"
            shift 2
            ;;
        --prepare-image)
            prepare_image=$2
            shift 2
            ;;
        --skip-build)
            skip_build=$2
            shift 2
            ;;
        --interactive)
            interactive=$2
            shift 2
            ;;
        --connect)
            connect=$2
            shift 2
            ;;
        --use-kvm)
            use_kvm=$2
            shift 2
            ;;
        --ram-size)
            ram_size=$2
            shift 2
            ;;
        --cpu-cores)
            cpu_cores=$2
            shift 2
            ;;
        --mount-vm-storage)
            mount_vm_storage=$2
            shift 2
            ;;
        --mount-client)
            mount_client=$2
            shift 2
            ;;
        --mount-server)
            mount_server=$2
            shift 2
            ;;
        --browser-port)
            browser_port="$2"
            shift 2
            ;;
        --rdp-port)
            rdp_port="$2"
            shift 2
            ;;
        --start-client)
            start_client=$2
            shift 2
            ;;
        --agent)
            agent=$2
            shift 2
            ;;
        --model)
            model=$2
            shift 2
            ;;
        --som-origin)
            som_origin=$2
            shift 2
            ;;
        --a11y-backend)
            a11y_backend=$2
            shift 2
            ;;
        --gpu-enabled)
            gpu_enabled=$2
            shift 2
            ;;
        --mode)
            mode=$2
            shift 2
            ;;
        --help)
            echo "Usage: $0 [options]"
            echo "Options:"
            echo "  --container-name <name> : Name of the arena container (default: winarena)"
            echo "  --prepare-image <true/false> : Prepare an arena golden image (default: false)"
            echo "  --skip-build <true/false> : Skip building the arena container image (default: false)"
            echo "  --interactive <true/false> : Launches the arena container in interactive mode, providing access to the command line (bin/bash) without initiating the client or VM server processes. (default: false)"
            echo "  --connect <true/false> : Whether to attach to an existing arena container, only if the container exists (default: false)"
            echo "  --use-kvm <true/false> : Whether to use KVM for VM acceleration (default: true)"
            echo "  --ram-size <ram_size> : RAM size for the VM (default: 8GB)"
            echo "  --cpu-cores <cpu_cores> : Number of CPU cores for the VM (default: 8)"
            echo "  --mount-vm-storage <true/false> : Mount the VM storage directory (default: true)"
            echo "  --mount-client <true/false> : Mount the client directory (default: true)"
            echo "  --mount-server <true/false> : Mount the server directory. Applies only for --mode dev. (default: true)"
            echo "  --browser-port <port> : Port to expose for connecting to the VM using browser (default: 8006)"
            echo "  --rdp-port <port> : Port to expose for connecting to the VM using RDP (default: 3390)"
            echo "  --start-client <true/false> : Whether to start the arena client process (default: true)"
            echo "  --agent <navi> : Agent to use for the arena container (default: navi)"
            echo "  --model <model>: The model to use (default: gpt-4-vision-preview, available options are: gpt-4o-mini, gpt-4-vision-preview, gpt-4o, gpt-4-1106-vision-preview)"
            echo "  --som-origin <som_origin>: The SoM (Set-of-Mark) origin to use (default: oss, available options are: oss, a11y, mixed-oss, omni, mixed-omni)"
            echo "  --a11y-backend <a11y_backend>: The a11y accessibility backend to use (default: uia, available options are: uia, win32)"
            echo "  --gpu-enabled <true/false> : Enable GPU support (default: false)"
            echo "  --mode <dev/azure> : Mode (default: azure)"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            log_error_exit "Unknown option: $1"
            ;;
    esac
done

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )

# Resolve paths
config_file_path="$SCRIPT_DIR/../config.json"

# Solve absolute path
config_file_path=$(getrealpath $config_file_path)
echo "Using configuration file: $config_file_path"
echo "Using mode: $mode"

OPENAI_API_KEY=$(extract_json_field_from_file "OPENAI_API_KEY" "$config_file_path")
AZURE_API_KEY=$(extract_json_field_from_file "AZURE_API_KEY" "$config_file_path")
AZURE_ENDPOINT=$(extract_json_field_from_file "AZURE_ENDPOINT" "$config_file_path")

# Check if at least one key has been set: OPENAI_API_KEY or both AZURE_API_KEY and AZURE_ENDPOINT
if [[ -z "$OPENAI_API_KEY" && (-z "$AZURE_API_KEY" || -z "$AZURE_ENDPOINT") ]]; then
    log_error_exit "Either OPENAI_API_KEY must be set or both AZURE_API_KEY and AZURE_ENDPOINT must be set: $1"
fi

./run.sh --mode $mode --prepare-image $prepare_image --container-name $container_name --skip-build $skip_build --interactive $interactive --connect $connect --use-kvm $use_kvm --ram-size $ram_size --cpu-cores $cpu_cores --mount-vm-storage $mount_vm_storage --mount-client $mount_client --mount-server $mount_server --browser-port $browser_port --rdp-port $rdp_port --start-client $start_client --agent $agent --model $model --som-origin $som_origin --a11y-backend $a11y_backend --gpu-enabled $gpu_enabled --openai-api-key $OPENAI_API_KEY --azure-api-key $AZURE_API_KEY --azure-endpoint $AZURE_ENDPOINT