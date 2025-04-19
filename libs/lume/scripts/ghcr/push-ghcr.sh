#!/bin/bash

# Exit immediately if a command exits with a non-zero status
set -e

# Default parameters
organization=""
folder_path=""
image_name=""
image_versions=""
chunk_size="500M"  # Default chunk size for splitting large files
# Define the OCI media type for the compressed disk layer
oci_layer_media_type="application/octet-stream+lzfse"  # Apple Archive format

# Parse the command line arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --organization)
            organization="$2"
            shift 2
            ;;
        --folder-path)
            folder_path="$2"
            shift 2
            ;;
        --image-name)
            image_name="$2"
            shift 2
            ;;
        --image-versions)
            image_versions="$2"
            shift 2
            ;;
        --chunk-size)
            chunk_size="$2"
            shift 2
            ;;
        --help)
            echo "Usage: $0 [options]"
            echo "Options:"
            echo "  --organization <organization>       : GitHub organization (required if not using token)"
            echo "  --folder-path <path>                : Path to the folder to upload (required)"
            echo "  --image-name <name>                 : Name of the image to publish (required)"
            echo "  --image-versions <versions>         : Comma separated list of versions of the image to publish (required)"
            echo "  --chunk-size <size>                 : Size of chunks for large files (e.g., 500M, default: 500M)"
            echo "Note: The script will automatically resume from the last attempt if available"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Ensure required arguments
if [[ -z "$organization" || -z "$folder_path" || -z "$image_name" || -z "$image_versions" ]]; then
    echo "Error: Missing required arguments. Use --help for usage."
    exit 1
fi

# Check if the GITHUB_TOKEN variable is set
if [[ -z "$GITHUB_TOKEN" ]]; then
    echo "Error: GITHUB_TOKEN is not set."
    exit 1
fi

# Ensure the folder exists
if [[ ! -d "$folder_path" ]]; then
    echo "Error: Folder $folder_path does not exist."
    exit 1
fi

# Check and install required tools
for tool in "oras" "split" "pv" "jq"; do
    if ! command -v "$tool" &> /dev/null; then
        echo "$tool is not installed. Installing using Homebrew..."
        if ! command -v brew &> /dev/null; then
            echo "Homebrew is not installed. Please install Homebrew first: https://brew.sh/"
            exit 1
        fi
        brew install "$tool"
    fi
done

# Check if Apple Archive is available
if ! command -v compression_tool &> /dev/null; then
    echo "Error: Apple Archive (compression_tool) is required but not found"
    echo "This script requires macOS with Apple Archive support"
    exit 1
fi

echo "Apple Archive detected - will use for optimal sparse file handling"
compressed_ext=".aa"

# Authenticate with GitHub Container Registry
echo "$GITHUB_TOKEN" | oras login ghcr.io -u "$organization" --password-stdin

# Use the source folder path as the working directory and get its absolute path
work_dir=$(cd "$folder_path" && pwd)
echo "Working directory: $work_dir"

# Function to find the most recent cache directory
find_latest_cache() {
    local latest_cache=$(ls -td "$work_dir"/.ghcr_cache_* 2>/dev/null | head -n1)
    if [ -n "$latest_cache" ]; then
        echo "$latest_cache"
    else
        echo ""
    fi
}

# Function to check if a cache directory is valid for resuming
is_valid_cache() {
    local cache_dir="$1"
    # Check if it contains the necessary files
    [ -f "$cache_dir/config.json" ] || [ -f "$cache_dir/nvram.bin" ] || \
    [ -f "$cache_dir/disk.img.aa" ] || ls "$cache_dir"/disk.img.aa.part.* 1>/dev/null 2>&1
}

# Always try to find and use an existing cache
existing_cache=$(find_latest_cache)
if [ -n "$existing_cache" ] && is_valid_cache "$existing_cache"; then
    cache_dir="$existing_cache"
    
    # Check if the cache contains old gzip format
    if [ -f "$cache_dir/disk.img.gz" ] || ls "$cache_dir"/disk.img.gz.part.* 1>/dev/null 2>&1; then
        echo "Error: Found legacy gzip format in cache. This script only supports Apple Archive format."
        echo "Please delete the cache directory and start fresh: $cache_dir"
        exit 1
    fi
    
    echo "Resuming from existing cache: $cache_dir"
else
    echo "No valid cache found. Starting fresh."
    cache_dir="$work_dir/.ghcr_cache_$(date +%Y%m%d_%H%M%S)"
    mkdir -p "$cache_dir"
fi

echo "Using cache directory: $cache_dir"

# Display space information
echo "=== DISK SPACE INFORMATION ==="
df -h "$cache_dir" | head -1
df -h "$cache_dir" | grep -v "Filesystem"
echo

# Change to the cache directory
cd "$cache_dir"
files=() # Initialize files array here

# Function to check if a version was already pushed
version_pushed() {
    local version="$1"
    local version_file="$cache_dir/.pushed_$version"
    [ -f "$version_file" ]
}

# Function to mark a version as pushed
mark_version_pushed() {
    local version="$1"
    touch "$cache_dir/.pushed_$version"
}

# Copy config.json if it exists and not already in cache
config_json_source="$folder_path/config.json"
config_json_dest="$cache_dir/config.json"
if [ -f "$config_json_source" ]; then
    if [ ! -f "$config_json_dest" ]; then
        echo "Copying config.json..."
        # Add the uncompressed disk size annotation if disk.img exists and jq is available
        if [ -n "$original_disk_size" ] && command -v jq &> /dev/null; then
             echo "Adding uncompressed disk size annotation: $original_disk_size bytes"
             jq --arg size "$original_disk_size" '.annotations += {"com.trycua.lume.disk.uncompressed_size": $size}' "$config_json_source" > "$config_json_dest" || \
                 (echo "jq failed, copying original config.json"; cp "$config_json_source" "$config_json_dest") # Fallback to copy if jq fails
        else
             cp "$config_json_source" "$config_json_dest"
        fi
    fi
fi
if [ -f "$config_json_dest" ]; then
    files+=("config.json:application/vnd.oci.image.config.v1+json")
fi

# Copy nvram.bin if it exists and not already in cache
if [ -f "$folder_path/nvram.bin" ] && [ ! -f "$cache_dir/nvram.bin" ]; then
    echo "Copying nvram.bin..."
    cp "$folder_path/nvram.bin" nvram.bin
fi
if [ -f "$cache_dir/nvram.bin" ]; then
    files+=("nvram.bin:application/octet-stream")
fi

# Process disk.img if it exists
disk_img_orig="$folder_path/disk.img"
original_disk_size=""
if [ -f "$disk_img_orig" ]; then
    # Get original size *before* compression
    original_disk_size=$(stat -f%z "$disk_img_orig")
    
    # Get real (non-sparse) size
    real_size=$(du -k "$disk_img_orig" | cut -f1)
    real_size_bytes=$((real_size * 1024))
    sparseness_ratio=$(echo "scale=2; $original_disk_size / $real_size_bytes" | bc)
    echo "Disk image: $disk_img_orig"
    echo "  Logical size: $original_disk_size bytes ($(du -h "$disk_img_orig" | cut -f1))"
    echo "  Actual disk usage: $((real_size_bytes / 1073741824)) GB"
    echo "  Sparseness ratio: ${sparseness_ratio}:1"
    
    # Check if we already have compressed files in the cache
    compressed_disk_img="disk.img${compressed_ext}"
    already_compressed=false
    
    if [ -f "$cache_dir/$compressed_disk_img" ]; then
        already_compressed=true
        echo "Using existing compressed file from cache: $compressed_disk_img"
    elif ls "$cache_dir"/disk.img${compressed_ext}.part.* 1>/dev/null 2>&1; then
        already_compressed=true
        echo "Using existing compressed parts from cache"
    fi

    # Only compress if not already compressed in cache
    if [ "$already_compressed" = false ]; then
        # Check for free disk space before compression
        avail_space=$(df -k "$cache_dir" | tail -1 | awk '{print $4}')
        avail_space_bytes=$((avail_space * 1024))
        # Assume compressed size is roughly 30% of real size as a safe estimate
        estimated_compressed=$((real_size_bytes * 30 / 100))
        
        if [ "$avail_space_bytes" -lt "$estimated_compressed" ]; then
            echo "WARNING: Possibly insufficient disk space for compression!"
            echo "Available: $((avail_space_bytes / 1073741824)) GB, Estimated required: $((estimated_compressed / 1073741824)) GB"
            read -p "Continue anyway? (y/n) " -n 1 -r
            echo
            if [[ ! $REPLY =~ ^[Yy]$ ]]; then
                echo "Exiting. Free up some space and try again."
                exit 1
            fi
        fi
    
        # --- Compression Step ---
        echo "Compressing $disk_img_orig with Apple Archive..."
        
        # Apple Archive compression
        echo "Starting compression with Apple Archive (showing output file growth)..."
        compression_tool -encode -i "$disk_img_orig" -o "$compressed_disk_img" -a lzfse &
        COMP_PID=$!
        
        sleep 1  # Give compression a moment to start
        
        # Display progress based on output file growth
        while kill -0 $COMP_PID 2>/dev/null; do
            if [ -f "$compressed_disk_img" ]; then
                current_size=$(stat -f%z "$compressed_disk_img" 2>/dev/null || echo 0)
                percent=$(echo "scale=2; 100 * $current_size / $original_disk_size" | bc)
                echo -ne "Progress: $percent% ($(du -h "$compressed_disk_img" 2>/dev/null | cut -f1 || echo "0"))\r"
            else
                echo -ne "Preparing compression...\r"
            fi
            sleep 2
        done
        
        wait $COMP_PID
        echo -e "\nCompression complete!"
        
        compressed_size=$(stat -f%z "$compressed_disk_img")
        echo "Compressed disk image size: $(du -h "$compressed_disk_img" | cut -f1)"
        echo "Compression ratio: $(echo "scale=2; $compressed_size * 100 / $original_disk_size" | bc)%"
        # --- End Compression Step ---

        # Check if splitting is needed based on *compressed* size
        if [ $compressed_size -gt 524288000 ]; then # 500MB threshold
            echo "Splitting compressed file into chunks of $chunk_size..."
            pv "$compressed_disk_img" | split -b "$chunk_size" - "$compressed_disk_img.part."
            rm -f "$compressed_disk_img"  # Remove the unsplit compressed file
            # Verify that parts were created
            echo "Verifying split parts..."
            ls -la "$cache_dir"/disk.img${compressed_ext}.part.*
        fi
    else
        echo "Using existing compressed/split files from cache"
    fi

    # --- Adjust part processing ---
    echo "Looking for compressed files in $cache_dir..."
    
    # List all files in the cache directory for debugging
    ls -la "$cache_dir"
    
    if [ -f "$cache_dir/$compressed_disk_img" ]; then
        echo "Found single compressed file: $compressed_disk_img"
        # Add the single compressed file to the list
        files+=("$compressed_disk_img:${oci_layer_media_type}")
    else
        # Look for split parts
        part_files=($(ls "$cache_dir"/disk.img${compressed_ext}.part.* 2>/dev/null || echo ""))
        if [ ${#part_files[@]} -gt 0 ]; then
            echo "Found ${#part_files[@]} split parts"
            parts_files=()
            part_num=0
            
            for part in "${part_files[@]}"; do
                part_num=$((part_num + 1))
                part_basename=$(basename "$part")
                parts_files+=("$part_basename:${oci_layer_media_type};part.number=$part_num;part.total=${#part_files[@]}")
                echo "Part $part_num: $(du -h "$part" | cut -f1)"
            done
            
            files+=("${parts_files[@]}")
        else
            echo "ERROR: No compressed files found in cache directory: $cache_dir"
            echo "Contents of cache directory:"
            find "$cache_dir" -type f | sort
            exit 1
        fi
    fi

    # --- Push Logic ---
    push_pids=()
    IFS=',' read -ra versions <<< "$image_versions"
    for version in "${versions[@]}"; do
        # Trim whitespace if any from version splitting
        version=$(echo "$version" | xargs)
        if [[ -z "$version" ]]; then continue; fi

        # Skip if version was already pushed
        if version_pushed "$version"; then
            echo "Version $version was already pushed, skipping..."
            continue
        fi

        echo "Pushing version $version..."
        (
            # Use process substitution to feed file list safely if it gets long
            oras push --disable-path-validation \
                "ghcr.io/$organization/$image_name:$version" \
                "${files[@]}"
            echo "Completed push for version $version"
            mark_version_pushed "$version"
        ) &
        push_pids+=($!)
    done

    # Wait for all pushes to complete
    for pid in "${push_pids[@]}"; do
        wait "$pid"
    done

    # --- Cleanup only if all versions were pushed successfully ---
    all_versions_pushed=true
    for version in "${versions[@]}"; do
        version=$(echo "$version" | xargs)
        if [[ -z "$version" ]]; then continue; fi
        if ! version_pushed "$version"; then
            all_versions_pushed=false
            break
        fi
    done

    if [ "$all_versions_pushed" = true ]; then
        echo "All versions pushed successfully, cleaning up cache directory..."
        cd "$work_dir"
        rm -rf "$cache_dir"
    else
        echo "Some versions failed to push. Cache directory preserved at: $cache_dir"
        echo "Run again to resume from this point"
    fi

else
    echo "Warning: $disk_img_orig not found."
    # Push only config/nvram if they exist
    if [ ${#files[@]} -gt 0 ]; then
        echo "Pushing non-disk files..."
        push_pids=()
        IFS=',' read -ra versions <<< "$image_versions"
        for version in "${versions[@]}"; do
            # Trim whitespace if any from version splitting
            version=$(echo "$version" | xargs)
            if [[ -z "$version" ]]; then continue; fi

            # Skip if version was already pushed
            if version_pushed "$version"; then
                echo "Version $version was already pushed, skipping..."
                continue
            fi

            echo "Pushing version $version (config/nvram only)..."
            (
                oras push --disable-path-validation \
                    "ghcr.io/$organization/$image_name:$version" \
                    "${files[@]}"
                echo "Completed push for version $version"
                mark_version_pushed "$version"
            ) &
            push_pids+=($!)
        done

        # Wait for all pushes to complete
        for pid in "${push_pids[@]}"; do
            wait "$pid"
        done

        # --- Cleanup only if all versions were pushed successfully ---
        all_versions_pushed=true
        for version in "${versions[@]}"; do
            version=$(echo "$version" | xargs)
            if [[ -z "$version" ]]; then continue; fi
            if ! version_pushed "$version"; then
                all_versions_pushed=false
                break
            fi
        done

        if [ "$all_versions_pushed" = true ]; then
            echo "All non-disk versions pushed successfully, cleaning up cache directory..."
            cd "$work_dir"
            rm -rf "$cache_dir"
        else
            echo "Some non-disk versions failed to push. Cache directory preserved at: $cache_dir"
            echo "Run again to resume from this point"
        fi
    else
        echo "No files found to push."
        cd "$work_dir"
        rm -rf "$cache_dir"
        exit 1
    fi
fi

# Determine final status based on the success check *before* potential cleanup
echo # Add a newline for better readability
if [ "$all_versions_pushed" = true ]; then
    echo "All versions pushed successfully:"
    for version in "${versions[@]}"; do
        version=$(echo "$version" | xargs)
        if [[ -z "$version" ]]; then continue; fi
        echo "  Upload complete: ghcr.io/$organization/$image_name:$version"
    done
else
    echo "Final upload status:"
    for version in "${versions[@]}"; do
        version=$(echo "$version" | xargs)
        if [[ -z "$version" ]]; then continue; fi
        # Check the marker file only if the overall process failed (cache preserved)
        if version_pushed "$version"; then
            echo "  Upload complete: ghcr.io/$organization/$image_name:$version"
        else
            echo "  Upload failed: ghcr.io/$organization/$image_name:$version"
        fi
    done
    # Exit with error code if any version failed
    exit 1
fi
