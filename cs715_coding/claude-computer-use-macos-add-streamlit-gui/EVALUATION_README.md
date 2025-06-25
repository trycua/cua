# Task Evaluation System

This system computes three key metrics for evaluating Claude Computer Use task performance:

1. **Task Success Rate (TSR)**: Did the agent complete the task correctly? (0.0 or 1.0)
2. **Step Success Rate (SSR)**: What percentage of steps were executed successfully? (0.0 to 1.0)  
3. **Action Efficiency (AE)**: How close was the plan to the optimal number of steps? (0.0 to 1.0)

## System Architecture

The evaluation system is structured into reusable modules:

### `utils.py` - Log Parser
- **`LogParser`**: Parses JSON log files into structured `Task` objects
- **`Task`**: Represents a complete task execution with steps, metadata, and results
- **`Step`**: Represents individual tool calls with success/failure status
- Helper functions for screenshot extraction and task summaries

### `metrics.py` - Metric Functions  
- **`MetricsCalculator`**: Computes TSR, SSR, and AE metrics
- **`EvaluationResult`**: Stores human evaluation data
- **`TaskMetrics`**: Stores computed metrics for a task
- Aggregation and reporting functions

### `run_eval.py` - Streamlit Interface
- **Human evaluation interface** for reviewing tasks and screenshots
- **Interactive forms** for evaluating task success and step failures
- **Metrics dashboard** with visualizations and export options
- **Storage management** for evaluations and computed metrics

### `eval_cli.py` - Command Line Tool
- **Batch processing** of log files
- **Sample evaluation generation** for testing
- **Metrics computation** without UI dependencies
- **JSON export** of results

## Usage

### Option 1: Streamlit Interface (Recommended)

1. Run the main Streamlit app:
```bash
streamlit run streamlit_app.py
```

2. Navigate to "Evaluate Tasks" page

3. For each task:
   - Review task details and screenshots
   - Read the final agent output
   - Evaluate whether task was successful
   - Mark any failed steps
   - Specify optimal number of steps
   - Add notes and save evaluation

4. View results in "Metrics Dashboard" with visualizations

### Option 2: Command Line Tool

1. Run the CLI evaluator:
```bash
python eval_cli.py
```

2. This will:
   - Parse all log files in `logs/` directory
   - Create sample evaluations (replace with real human evaluations)
   - Compute all metrics
   - Save results to `evaluations/` directory

## Metric Definitions

### Task Success Rate (TSR)
- **Definition**: Binary success indicator (1.0 if successful, 0.0 if failed)
- **Evaluation**: Based on human assessment of whether the overall task goal was achieved
- **Example**: Calculator task asking for "123 + 456" → TSR = 1.0 if final answer is 579

### Step Success Rate (SSR)  
- **Definition**: `(Total Steps - Failed Steps) / Total Steps`
- **Failed Steps**: Steps with technical errors OR marked as failed by human evaluator
- **Example**: 5 total steps, 1 error, 0 human-marked failures → SSR = 4/5 = 0.8

### Action Efficiency (AE)
- **Definition**: `min(Optimal, Actual) / max(Optimal, Actual)`
- **Actions**: Only non-screenshot steps count (screenshots are for observation)
- **Optimal**: Human-specified ideal number of steps
- **Example**: 8 actual actions, 6 optimal → AE = 6/8 = 0.75

## File Structure

```
logs/                          # JSON log files from experiments
evaluations/                   # Evaluation results storage
├── evaluations.json          # Human evaluation data
└── metrics.json              # Computed metrics
screenshots/                   # Screenshot images
utils.py                      # Log parsing utilities
metrics.py                    # Metric calculation functions
run_eval.py                   # Streamlit evaluation interface
eval_cli.py                   # Command-line evaluation tool
streamlit_app.py              # Main application (updated)
```

## Data Flow

1. **Log Generation**: `streamlit_app.py` creates structured JSON logs during task execution
2. **Log Parsing**: `utils.py` converts JSON logs into `Task` objects  
3. **Human Evaluation**: `run_eval.py` provides interface for human review
4. **Metric Computation**: `metrics.py` calculates TSR, SSR, AE from evaluations
5. **Results Storage**: Evaluations and metrics saved as JSON files
6. **Analysis**: Dashboard provides visualizations and export options

## Key Features

- ✅ **Human-in-the-loop evaluation** with screenshot review
- ✅ **Structured metric definitions** aligned with research standards  
- ✅ **Reusable modular architecture** for different evaluation workflows
- ✅ **Interactive dashboard** with visualizations and export
- ✅ **Batch processing** support via CLI tool
- ✅ **JSON storage** for integration with other tools
- ✅ **Error handling** and validation throughout

## Example Output

```
=== AGGREGATE METRICS REPORT ===

Tasks Evaluated: 3
Successful Tasks: 2

Average Task Success Rate (TSR): 66.7%
Average Step Success Rate (SSR): 85.4%
Average Action Efficiency (AE): 78.2%

Task Success Rate: 2/3 = 66.7%
```

This system provides comprehensive evaluation capabilities for Claude Computer Use tasks with both interactive and batch processing options.
