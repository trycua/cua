#!/bin/bash

agent="navi"
model="gpt-4-vision-preview"
som_origin="oss"
a11y_backend="uia"
clean_results=true
worker_id="0"
num_workers="1"
result_dir="./results"
json_name="evaluation_examples_windows/test_all.json"
diff_lvl="normal"

# parse agent argument
while [[ $# -gt 0 ]]; do
    case "$1" in
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
        --clean-results)
            clean_results=$2
            shift 2
            ;;
        --worker-id)
            worker_id=$2
            shift 2
            ;;
        --num-workers)
            num_workers=$2
            shift 2
            ;;
        --result-dir)
            result_dir=$2
            shift 2
            ;;
        --json-name)
            json_name=$2
            shift 2
            ;;
        --diff-lvl)
            diff_lvl=$2  
            shift 2  
            ;;              
        --help)
            echo "Usage: $0 [options]"
            echo "Options:"
            echo "  --agent <agent>                 The agent to use (default: navi)"
            echo "  --model <model>                 The model to use (default: gpt-4-vision-preview, available options are: gpt-4o-mini, gpt-4-vision-preview, gpt-4o, gpt-4-1106-vision-preview)"
            echo "  --som-origin <som_origin>       The SoM (Set-of-Mark) origin to use (default: oss, available options are: oss, a11y, mixed-oss)"
            echo "  --a11y-backend <a11y_backend>   The a11y accessibility backend to use (default: uia, available options are: uia, win32)"
            echo "  --clean-results <bool>          Clean the results directory before running the client (default: true)"
            echo "  --worker-id <id>                The worker ID"
            echo "  --num-workers <num>             The number of workers"
            echo "  --result-dir <dir>              The directory to store the results (default: ./results)"
            echo "  --json-name <name>              The name of the JSON file to use (default: test_all.json)"
            echo "  --diff-lvl <level>              The difficulty level of benchmark (default: normal, available options are: normal, hard)"  
            exit 0
            ;;
        *)
    esac
done

cd /client
if [ "$clean_results" = true ]; then
    echo "Cleaning results directory..."
    rm -rf "$result_dir"/*
fi

echo "Running agent $agent..."
python run.py --agent "$agent" --model "$model" --som_origin "$som_origin" --a11y_backend "$a11y_backend" --worker_id "$worker_id" --num_workers "$num_workers" --result_dir "$result_dir" --test_all_meta_path "$json_name" --diff_lvl "$diff_lvl"