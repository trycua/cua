# Function to log error message and exit
log_error_exit() {
    echo "Error: $1" >&2
    exit 1
}

# Resolve the path of a file or directory
getrealpath () (
    if [[ -d $1 ]]; then
        OLDPWD=- CDPATH= cd -P -- "$1" && pwd
    else
        OLDPWD=- CDPATH= cd -P -- "${1%/*}" && printf '%s/%s\n' "$PWD" "${1##*/}"
    fi
)

# Core function to extract a JSON field value, only from the first level of the JSON structure using regex
extract_json_field() {
    local field_name=$1
    local input=$2
    # Use regex to extract the field value at the first JSON level
    local result
    result=$(echo "$input" | grep -oP '"'"$field_name"'"\s*:\s*"\K[^"]+')
    if [[ $? -ne 0 ]]; then
        echo ""
    else
        echo "$result"
    fi
}

# Extract a JSON field from a JSON file using regex
extract_json_field_from_file() {
    local field_name=$1
    local json_file=$2
    # Read the file content and use regex to extract the field
    local json_text
    json_text=$(<"$json_file")
    extract_json_field "$field_name" "$json_text"
}

# Extract a JSON field value from JSON text using regex
extract_json_field_from_text() {
    local field_name=$1
    local json_text=$2
    extract_json_field "$field_name" "$json_text"
}