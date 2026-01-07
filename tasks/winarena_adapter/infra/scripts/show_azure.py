import os
import json
import pandas as pd
import argparse
import numpy as np
from tqdm import tqdm


def get_results_from_json(result_dir, config, output_file):
    columns = ["exp_name", "uia_value", "som_origin", "model",
               "chrome", "libreoffice_calc", "libreoffice_writer", 
               "vlc", "vs_code", "settings", "windows_calc", 
               "clock", "msedge", "file_explorer", "microsoft_paint", "notepad", "*errors*"]
    
    # Start the markdown file and write the headers
    with open(output_file, "w") as f:
        f.write("| " + " | ".join(columns) + " |\n")
        f.write("|" + " | ".join(["---"] * len(columns)) + "|\n")
    
    # Iterate through each experiment configuration
    for exp_name, exp_details in tqdm(config.items(), desc="Processing Experiments"):
        row = {col: None for col in columns}
        row["exp_name"] = exp_name
        row["uia_value"] = "✅" if exp_details.get("a11y_backend", "") == "uia" else "❌"
        row["som_origin"] = exp_details.get("som_origin", "")
        row["model"] = exp_details.get("model_name", "")

        path_kwargs = {
            "result_dir": result_dir,
            "exp_name": exp_name,
            "action_space": "pyautogui",
            "observation_type": "a11y_tree",
            "model": exp_details["model_name"],
            "trial_id": "0"
        }
        
        path_args = [path_kwargs[k] for k in [
            "result_dir", "exp_name", "action_space", "observation_type", "model", "trial_id"
        ]]
        
        results_path = os.path.join(*path_args)
        errs = 0

        for domain in ["chrome", "libreoffice_calc", "libreoffice_writer", 
                       "vlc", "vs_code", "settings", "windows_calc", 
                       "clock", "msedge", "file_explorer", "microsoft_paint", "notepad"]:
            domain_path = os.path.join(results_path, domain)
            if os.path.isdir(domain_path):
                task_results = []
                for task in os.listdir(domain_path):
                    task_path = os.path.join(domain_path, task, "result.txt")
                    if os.path.isfile(task_path):
                        try:
                            with open(task_path, "r") as f:
                                result = float(f.read().strip())
                            task_results.append(result)
                        except ValueError:
                            print(f"Invalid result.txt in {exp_name}, {domain}/{task}")
                            errs += 1
                            task_results.append(np.nan)
                    else:
                        print(f"Missing result.txt in {exp_name}, {domain}/{task}")
                        errs += 1
                
                # Calculate the percentage of valid results
                if task_results:
                    valid_results = [res for res in task_results if not np.isnan(res)]
                    row[domain] = sum(valid_results) / len(valid_results) * 100 if valid_results else None
                else:
                    row[domain] = None
            else:
                row[domain] = None

        row["*errors*"] = errs
        
        # Convert row to markdown format and append to file
        with open(output_file, "a") as f:
            row_markdown = "| " + " | ".join([str(row[col]) if row[col] is not None else "" for col in columns]) + " |\n"
            f.write(row_markdown)
    
    print(f"Results table saved to {output_file}")

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Generate results table from JSON config.")
    parser.add_argument("--result_dir", type=str, required=True, help="Directory containing result files.")
    parser.add_argument("--json_config", type=str, required=True, help="Path to JSON config file.")
    parser.add_argument("--output_file", type=str, default="results_table.md", help="Output markdown file.")

    args = parser.parse_args()

    with open(args.json_config, "r") as f:
        config = json.load(f)

    get_results_from_json(args.result_dir, config, args.output_file)