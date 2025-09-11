# Minimal Example Agent

A tiny example showing how to run an agent inside a container:
- Creates `input.txt` if missing
- Reads it, uppercases content
- Writes the result to `output.txt`

## Run

### Linux/macOS
```bash
docker build -t cua-agent-example examples/minimal-agent
docker run --rm -v $(pwd)/examples/minimal-agent:/app cua-agent-example
