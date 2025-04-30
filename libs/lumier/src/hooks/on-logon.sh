setup_script="$DATA_FOLDER_PATH/setup.sh"

if [ -f "$setup_script" ]; then
    chmod +x "$setup_script"
    source "$setup_script"
else
    echo "Setup script not found at: $setup_script"
fi