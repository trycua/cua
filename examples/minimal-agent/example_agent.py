import os

INPUT_FILE = "input.txt"
OUTPUT_FILE = "output.txt"

def run_agent():
    if not os.path.exists(INPUT_FILE):
        with open(INPUT_FILE, "w") as f:
            f.write("Hello from Cua!\n")

    with open(INPUT_FILE, "r") as f:
        data = f.read()

    processed = data.upper()

    with open(OUTPUT_FILE, "w") as f:
        f.write(processed)

    print(f"Processed file written to {OUTPUT_FILE}")

if __name__ == "__main__":
    run_agent()
