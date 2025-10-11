#!/bin/bash

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m' # No Color

# Print with color
print_info() {
    echo -e "${BLUE}==> $1${NC}"
}

print_success() {
    echo -e "${GREEN}==> $1${NC}"
}

print_error() {
    echo -e "${RED}==> $1${NC}"
}

# Docker image name
IMAGE_NAME="cua-dev-image"
CONTAINER_NAME="cua-dev-container"
PLATFORM="linux/arm64"

# Detect platform based on architecture
arch=$(uname -m)

if [[ $arch == x86_64* ]]; then
    PLATFORM="linux/amd64"
    print_info "X64 Architecture detected, using platform: ${PLATFORM}"
elif [[ $arch == i*86 ]]; then
    PLATFORM="linux/386"
    print_info "X32 Architecture detected, using platform: ${PLATFORM}"
elif [[ $arch == arm* ]] || [[ $arch == aarch64 ]]; then
    PLATFORM="linux/arm64"
    print_info "ARM Architecture detected, using platform: ${PLATFORM}"
else
    # Fallback to amd64 for unknown architectures
    PLATFORM="linux/amd64"
    print_info "Unknown architecture ($arch), defaulting to platform: ${PLATFORM}"
fi

# Environment variables
PYTHONPATH="/app/libs/python/core:/app/libs/python/computer:/app/libs/python/agent:/app/libs/python/som:/app/libs/python/computer-server:/app/libs/python/mcp-server"

# Check if Docker is installed
if ! command -v docker &> /dev/null; then
    print_error "Docker is not installed. Please install Docker first."
    exit 1
fi

# Command options
case "$1" in
    build)
        print_info "Building the development Docker image..."
        print_info "This will install all dependencies but won't include source code"
        docker build -f Dockerfile --platform=${PLATFORM} -t ${IMAGE_NAME} .
        print_success "Development Docker image built successfully!"
        ;;
    
    run)
        # Check for interactive flag
        if [ "$2" == "--interactive" ]; then
            print_info "Running the development Docker container with interactive shell..."
            print_info "Mounting source code from host"
            print_info "Connecting to host.docker.internal:7777"
            
            docker run -it --rm \
                --platform=${PLATFORM} \
                --name ${CONTAINER_NAME} \
                -v "$(pwd):/app" \
                -e PYTHONPATH=${PYTHONPATH} \
                -e DISPLAY=${DISPLAY:-:0} \
                -e PYLUME_HOST="host.docker.internal" \
                -p 7860:7860 \
                ${IMAGE_NAME} bash
        else
            # Run the specified example
            if [ -z "$2" ]; then
                print_error "Please specify an example file, e.g., ./run-docker-dev.sh run computer_examples.py"
                exit 1
            fi
            print_info "Running example: $2"
            print_info "Connecting to host.docker.internal:7777"
            
            docker run -it --rm \
                --platform=${PLATFORM} \
                --name ${CONTAINER_NAME} \
                -v "$(pwd):/app" \
                -e PYTHONPATH=${PYTHONPATH} \
                -e DISPLAY=${DISPLAY:-:0} \
                -e PYLUME_HOST="host.docker.internal" \
                -p 7860:7860 \
                ${IMAGE_NAME} python "/app/examples/$2"
        fi
        ;;
    
    stop)
        print_info "Stopping any running containers..."
        docker stop ${CONTAINER_NAME} 2>/dev/null || true
        print_success "Done!"
        ;;
        
    *)
        echo "Usage: $0 {build|run [--interactive] [filename]|stop}"
        echo ""
        echo "Commands:"
        echo "  build                      Build the development Docker image with dependencies"
        echo "  run [example_filename]     Run the specified example file in the container"
        echo "  run --interactive          Run the container with mounted code and get an interactive shell"
        echo "  stop                       Stop the container"
        exit 1
esac

exit 0 