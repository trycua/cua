# Claude Computer Use Demo for macOS with Evaluation System

This repository contains a Python script that demonstrates Anthropic's Computer Use capabilities, modified to run on macOS without requiring a Docker container. The script allows Claude 3.5 Sonnet to perform tasks on your Mac by simulating mouse and keyboard actions as well as running bash commands.

**🆕 NEW**: This repository now includes a comprehensive **Evaluation System** for analyzing Claude Computer Use performance with human-in-the-loop evaluation, metrics calculation, and interactive dashboards.

Forked from Anthropic's [computer use demo](https://github.com/anthropics/anthropic-quickstarts/tree/main/computer-use-demo) - optimized for macOS.
View Anthropic's docs [here](https://docs.anthropic.com/en/docs/build-with-claude/computer-use).

> [!WARNING]  
> Use this script with caution. Allowing Claude to control your computer can be risky. By running this script, you assume all responsibility and liability.

## Features

### Claude Computer Use Demo
- **Native macOS support** - No Docker required
- **Streamlit GUI** - Modern web interface for interacting with Claude
- **Screenshot capture** - Visual feedback of Claude's actions
- **JSON logging** - Complete session recordings for analysis

### Evaluation System
- **Human-in-the-loop evaluation** - Review and annotate Claude's performance
- **Comprehensive metrics** - Task Success Rate (TSR), Step Success Rate (SSR), Action Efficiency (AE)
- **Interactive dashboards** - Visualize performance trends and patterns
- **Retry management** - Handle failed tasks with retry mechanisms
- **Export capabilities** - Generate reports in JSON and CSV formats
- **CLI tools** - Batch processing and automation

## Installation and Setup

### Prerequisites
- **Python 3.8+** (Python 3.12 recommended)
- **macOS** (tested on macOS 10.15+)
- **Anthropic API key** - Get yours [here](https://console.anthropic.com/settings/keys)

### 1. Clone the repository

```bash
git clone https://github.com/PallavAg/claude-computer-use-macos.git
cd claude-computer-use-macos
```

### 2. Create virtual environment and install dependencies

```bash
python3 -m venv venv
source venv/bin/activate
pip install -r requirements.txt
```

### 3. Set up environment variables

Create a `.env` file in the root directory:

```bash
echo "ANTHROPIC_API_KEY=your_api_key_here" > .env
```

Alternatively, export the environment variable:

```bash
export ANTHROPIC_API_KEY="your_api_key_here"
```

### 4. Grant accessibility permissions

The script uses `pyautogui` to control mouse and keyboard events. On macOS, you need to grant accessibility permissions:

- Go to **System Preferences** > **Security & Privacy** > **Privacy** tab
- Select **Accessibility** from the list on the left
- Add your terminal application or Python interpreter to the list of allowed apps

Permissions popups should appear automatically the first time you run the script.

### 5. Create required directories

```bash
mkdir -p logs screenshots evaluations
```

## Usage

### Running Claude Computer Use Demo

#### Option 1: Streamlit GUI (Recommended)

```bash
streamlit run streamlit_app.py
```

This launches a web interface where you can:
- Enter instructions for Claude
- Monitor real-time execution
- View screenshots and responses
- Access session logs

#### Option 2: Command Line

```bash
python main.py "Your instruction here"
```

Example:
```bash
python main.py "Open Safari and search for the weather"
```

### Using the Evaluation System

#### Option 1: Interactive Web Interface

Launch the evaluation dashboard:

```bash
streamlit run run_eval.py
```

The evaluation interface provides:

- **📋 Evaluate Tasks**: Human-in-the-loop evaluation with step-by-step review
- **📊 Metrics Dashboard**: Interactive visualizations of performance metrics
- **🔄 Retry Management**: Handle failed tasks and manage retry workflows
- **📈 Analytics**: Detailed performance analysis and trend visualization

#### Option 2: Command Line Interface

For batch processing and automation:

```bash
# Evaluate all logs in the logs directory
python eval_cli.py --logs-dir logs --output evaluations/batch_eval.json

# Generate metrics report
python eval_cli.py --logs-dir logs --metrics-only --output evaluations/metrics_report.json

# Evaluate specific log files
python eval_cli.py --log-files logs/session1.json logs/session2.json --output evaluations/specific_eval.json
```

## Evaluation System Components

### Core Modules

- **`utils.py`**: Log parsing, task extraction, and data utilities
- **`metrics.py`**: Metrics calculation (TSR, SSR, AE) and aggregation
- **`run_eval.py`**: Streamlit web interface for human evaluation
- **`eval_cli.py`**: Command-line tool for batch processing
- **`streamlit_app.py`**: Main application entry point

### Metrics Explained

- **Task Success Rate (TSR)**: Percentage of tasks completed successfully
- **Step Success Rate (SSR)**: Percentage of individual steps executed correctly
- **Action Efficiency (AE)**: Ratio of optimal steps to actual steps taken

### Directory Structure

```
├── logs/                    # JSON log files from Claude sessions
├── screenshots/             # Screenshots captured during execution
├── evaluations/            # Human evaluation results and metrics
├── utils.py               # Log parsing and data utilities
├── metrics.py             # Metrics calculation functions
├── run_eval.py            # Streamlit evaluation interface
├── eval_cli.py            # Command-line evaluation tool
├── streamlit_app.py       # Main Streamlit application
└── requirements.txt       # Python dependencies
```

## Advanced Usage

### Custom Evaluation Workflows

1. **Run Claude sessions** using the main application
2. **Review logs** in the `logs/` directory
3. **Evaluate performance** using the web interface or CLI
4. **Generate reports** and export results
5. **Analyze trends** using the metrics dashboard

### Scientific Research

The evaluation system is designed for research purposes. See `EVALUATION_SYSTEM_DOCUMENTATION.md` for:
- Detailed methodology
- Metrics definitions
- Reproducibility guidelines
- Best practices for evaluation

### Automation and CI/CD

Use the CLI tools in automated pipelines:

```bash
# Automated evaluation script
#!/bin/bash
python eval_cli.py --logs-dir logs --output evaluations/nightly_$(date +%Y%m%d).json
python -c "from metrics import *; print(format_aggregate_report(...))"
```

## Troubleshooting

### Common Issues

1. **Permission denied**: Ensure accessibility permissions are granted
2. **Module not found**: Activate virtual environment and install dependencies
3. **API key errors**: Verify your Anthropic API key is set correctly
4. **Log parsing issues**: Check that log files are valid JSON format

### Getting Help

- Review the evaluation system documentation
- Check the logs for detailed error messages
- Ensure all dependencies are installed
- Verify file permissions and directory structure

## Security and Privacy

> [!CAUTION]
> - **Computer Control**: This script allows Claude to control your computer's mouse and keyboard
> - **Command Execution**: Claude can run bash commands on your system
> - **Data Logging**: Sessions are logged with screenshots for evaluation
> - **API Usage**: Your instructions are sent to Anthropic's servers
> - **Responsibility**: You assume all responsibility and liability for results

## Contributing

Contributions are welcome! Please:
1. Fork the repository
2. Create a feature branch
3. Add tests for new functionality
4. Update documentation
5. Submit a pull request

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Acknowledgments

- Based on Anthropic's computer use demo
- Evaluation system designed for research and analysis
- Community contributions and feedback

---

**📚 For detailed evaluation system documentation, see [`EVALUATION_SYSTEM_DOCUMENTATION.md`](EVALUATION_SYSTEM_DOCUMENTATION.md)**
