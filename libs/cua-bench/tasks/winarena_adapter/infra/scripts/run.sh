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
OPENAI_API_KEY=""
AZURE_API_KEY=""
AZURE_ENDPOINT=""

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
        --openai-api-key)
            OPENAI_API_KEY="$2"
            shift 2
            ;;
        --azure-api-key)
            AZURE_API_KEY="$2"
            shift 2
            ;;
        --azure-endpoint)
            AZURE_ENDPOINT="$2"
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
            echo "  --openai-api-key <key> : The OpenAI API key"
            echo "  --azure-api-key <key> : The Azure OpenAI API key"
            echo "  --azure-endpoint <url> : The Azure OpenAI Endpoint"
            echo "  --mode <dev/azure> : Mode (default: azure)"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            log_error_exit "Unknown option: $1"
            ;;
    esac
done

# Static parameters
if [ "$mode" = "dev" ]; then # Only for dev mode
  winarena_image_name="winarena-$mode"
else
  winarena_image_name="winarena"
fi

winarena_image_tag="latest" 
winarena_full_image_name="trycua/$winarena_image_name"

# Check if Docker daemon is running
if ! docker info >/dev/null 2>&1; then
    log_error_exit "Docker daemon is not running. Please start Docker and try again."
fi

# Check if the container image exists
if ! docker images | grep -q -e $winarena_full_image_name; then
    echo "Docker image $winarena_full_image_name not found."
    if [ "$skip_build" = true ]; then
        log_error_exit "The 'skip_build' flag is set to true, but the image $winarena_full_image_name was not found. To build the image, set 'skip_build' to false, or pull the latest image from the registry using: docker pull $winarena_full_image_name:$winarena_image_tag"
    fi
fi

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )

# Resolve paths
vm_setup_image_path="$SCRIPT_DIR/../src/win-arena-container/vm/image"
vm_storage_mount_path="$SCRIPT_DIR/../src/win-arena-container/vm/storage"
server_mount_path="$SCRIPT_DIR/../src/win-arena-container/vm/setup"
client_mount_path="$SCRIPT_DIR/../src/win-arena-container/client"

# Solve absolute path
vm_setup_image_path=$(getrealpath $vm_setup_image_path)
vm_storage_mount_path=$(getrealpath $vm_storage_mount_path)
server_mount_path=$(getrealpath $server_mount_path)
client_mount_path=$(getrealpath $client_mount_path)

echo "Using VM Setup Image path: $vm_setup_image_path"
echo "Using VM storage mount path: $vm_storage_mount_path"
echo "Using server mount path: $server_mount_path"
echo "Using client mount path: $client_mount_path"

# Check if /dev/kvm exists
if [ ! -e /dev/kvm ]; then
    echo "/dev/kvm not found. Setting use_kvm to false."
    use_kvm=false
fi

# Check if at least one key has been set: OPENAI_API_KEY or both AZURE_API_KEY and AZURE_ENDPOINT
if [[ -z "$OPENAI_API_KEY" && (-z "$AZURE_API_KEY" || -z "$AZURE_ENDPOINT") ]]; then
    log_error_exit "Either OPENAI_API_KEY must be set or both AZURE_API_KEY and AZURE_ENDPOINT must be set: $1"
fi

# Function to build container image
build_container_image() {
    echo "Building Container Image..."
    source "$SCRIPT_DIR/build-container-image.sh" --mode $mode
}

# Function to invoke Docker container
invoke_docker_container() {
    docker_command="docker run"

    # Add interactive and TTY flags
    if [ -t 1 ]; then
        docker_command+=" -it"
    fi

    # Ensure the container is removed after it exits
    docker_command+=" --rm"

    # Map ports from the container to the host
    docker_command+=" -p ${browser_port}:8006"
    docker_command+=" -p ${rdp_port}:3389"

    # Set the container name
    docker_command+=" --name $container_name"

    # Set the platform
    docker_command+=" --platform linux/amd64"

    # Add KVM
    if [ "$use_kvm" = true ]; then
        docker_command+=" --device=/dev/kvm"
    else
        docker_command+=" -e KVM=N"
    fi

    # Set the RAM size
    docker_command+=" -e RAM_SIZE=$ram_size"

    # Set the CPU cores
    docker_command+=" -e CPU_CORES=$cpu_cores"

    # Mount the setup image
    if [ "$prepare_image" = true ]; then
        docker_command+=" --mount type=bind,source=${vm_setup_image_path}/setup.iso,target=/custom.iso"
    fi

    # Mount the storage for the VM - makes the VM persistent
    if [ "$mount_vm_storage" = true ]; then
        docker_command+=" -v ${vm_storage_mount_path}/.:/storage"
    fi

    # Mount the shared directory between host and the Windows VM
    if [ "$mount_server" = true ]; then
        docker_command+=" -v ${server_mount_path}/.:/shared"
    fi

    # Mount the directory for the client process
    if [ "$mount_client" = true ]; then
        docker_command+=" -v ${client_mount_path}/.:/client"
    fi

    # Add network capabilities and set the stop timeout
    docker_command+=" --cap-add NET_ADMIN --stop-timeout 120"

    # Set the entrypoint to /bin/bash
    docker_command+=" --entrypoint /bin/bash"

    # Check if gpu is available if nvidia-container-toolkit is installed
    if [ "$gpu_enabled" = true ] && [ "$(command -v nvidia-smi)" ]; then
        docker_command+=" --gpus all"
    fi

    # OpenAI API Key priotitized over Azure API Key
    if [ -n "$OPENAI_API_KEY" ]; then
        docker_command+=" -e OPENAI_API_KEY=$OPENAI_API_KEY"
    else
        if [ -n "$AZURE_API_KEY" ]; then
            docker_command+=" -e AZURE_API_KEY=$AZURE_API_KEY"
        fi

        if [ -n "$AZURE_ENDPOINT" ]; then
            docker_command+=" -e AZURE_ENDPOINT=$AZURE_ENDPOINT"
        fi
    fi

    # Add the image name with tag
    docker_command+=" $winarena_full_image_name:$winarena_image_tag"
    
    # Set the entrypoint arguments
    entrypoint_args=" -c './entry.sh --prepare-image $prepare_image --start-client $start_client --agent $agent --model $model --som-origin $som_origin --a11y-backend $a11y_backend'"
    if [ "$interactive" = true ]; then
        entrypoint_args=""
    fi
    docker_command+=$entrypoint_args

    echo "Invoking Docker Container with the command:"
    echo "$docker_command"

    eval $docker_command
}

# if connect is true, attach to an existing container
if [ "$connect" = true ]; then
    echo "Connecting to existing container $container_name..."
    
    docker_exec_command="docker exec"
    # Add interactive and TTY flags
    if [ -t 1 ]; then
        docker_exec_command+=" -it"
    fi

    docker_exec_command+=" $container_name /bin/bash"
    echo "Invoking docker exec with the command:"
    echo "$docker_exec_command"

    eval $docker_exec_command
    exit 0
fi

# Build container image if not skipped
if [ "$skip_build" = false ]; then
    build_container_image
fi

invoke_docker_container
