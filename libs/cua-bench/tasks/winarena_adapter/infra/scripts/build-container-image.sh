#!/bin/bash

# Exit immediately if a command exits with a non-zero status
set -e

source ./shared.sh

build_base_image=false

# Parse the command line arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --build-base-image)
            build_base_image=$2
            shift 2
            ;;
        --help)
            echo "Usage: $0 [options]"
            echo "Options:"
            echo "  --build-base-image <true/false> : Whether to build the winarena-base image (default: false)"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            log_error_exit "Unknown option: $1"
            ;;
    esac
done

SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )

echo "$SCRIPT_DIR/../"

if [ "$build_base_image" = true ]; then
  echo "Building winarena-base image"
  docker build --build-arg PROFILE_MODE=false -f $SCRIPT_DIR/../src/win-arena-container/Dockerfile-WinArena-Base -t winarena-base:latest $SCRIPT_DIR/../
  docker tag winarena-base:latest trycua/winarena-base:latest
else
  echo "Skipping winarena-base image build"
fi

docker build -f $SCRIPT_DIR/../src/win-arena-container/Dockerfile-WinArena -t winarena:latest $SCRIPT_DIR/../

docker tag winarena:latest trycua/winarena:latest
