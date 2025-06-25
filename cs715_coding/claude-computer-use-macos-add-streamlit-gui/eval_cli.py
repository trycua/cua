"""
Command-line evaluation tool for computing task metrics.
"""

import json
import os
from typing import List, Dict
from utils import LogParser, Task, get_unique_task_id
from metrics import MetricsCalculator, EvaluationResult, TaskMetrics, aggregate_metrics, format_metrics_report, format_aggregate_report


def load_evaluations(evaluations_file: str) -> Dict[str, EvaluationResult]:
    """Load evaluations from JSON file."""
    if not os.path.exists(evaluations_file):
        return {}
    
    try:
        with open(evaluations_file, 'r') as f:
            data = json.load(f)
        
        evaluations = {}
        for task_id, eval_data in data.items():
            evaluations[task_id] = EvaluationResult(**eval_data)
        
        return evaluations
    except Exception as e:
        print(f"Error loading evaluations: {e}")
        return {}


def create_sample_evaluation(task: Task) -> EvaluationResult:
    """Create a sample evaluation for demonstration purposes."""
    print(f"\n=== EVALUATING TASK: {get_unique_task_id(task)} ===")
    print(f"Test ID: {task.test_id}, Session: {task.session_id}")
    print(f"Instruction: {task.instruction}")
    print(f"Steps: {len(task.steps)} total ({task.action_count} actions)")
    print(f"Duration: {task.duration:.1f}s")
    print(f"Final output: {task.final_output[:100]}...")
    
    # Simple heuristic evaluation (replace with actual human evaluation)
    task_successful = (task.status == "completed" and 
                      len(task.final_output) > 0 and
                      task.action_count > 0)
    
    # Assume minimal step failures for demo
    step_failures = [step.step_id for step in task.steps if not step.is_successful]
    
    # Estimate optimal steps (simplified heuristic)
    if task.action_count > 0:
        optimal_steps = max(1, task.action_count - 1)  # Assume one less than actual
    else:
        optimal_steps = 1  # Minimum one step for any task
    
    return EvaluationResult(
        task_id=get_unique_task_id(task),
        task_successful=task_successful,
        step_failures=step_failures,
        optimal_step_count=optimal_steps,
        notes=f"Auto-generated evaluation. Status: {task.status}, Actions: {task.action_count}",
        evaluated_by="CLI_Script",
        evaluation_date="2025-06-25T00:00:00"
    )


def main():
    """Main function for CLI evaluation."""
    print("🔍 Task Evaluation System")
    print("=" * 50)
    
    # Initialize components
    logs_dir = "logs"
    evaluations_dir = "evaluations"
    os.makedirs(evaluations_dir, exist_ok=True)
    
    parser = LogParser(logs_dir)
    calculator = MetricsCalculator()
    
    # Parse all tasks
    tasks = parser.parse_all_logs()
    if not tasks:
        print("❌ No log files found in 'logs' directory!")
        return
    
    print(f"📋 Found {len(tasks)} tasks to evaluate")
    
    # Load existing evaluations
    evaluations_file = os.path.join(evaluations_dir, "evaluations.json")
    evaluations = load_evaluations(evaluations_file)
    
    print(f"✅ Found {len(evaluations)} existing evaluations")
    
    # Create sample evaluations for missing tasks
    for task in tasks:
        unique_task_id = get_unique_task_id(task)
        if unique_task_id not in evaluations:
            evaluation = create_sample_evaluation(task)
            evaluations[unique_task_id] = evaluation
            print(f"📝 Created evaluation for task {unique_task_id}")
    
    # Save evaluations
    eval_data = {}
    for task_id, evaluation in evaluations.items():
        eval_data[task_id] = {
            "task_id": evaluation.task_id,
            "task_successful": evaluation.task_successful,
            "step_failures": evaluation.step_failures,
            "optimal_step_count": evaluation.optimal_step_count,
            "notes": evaluation.notes,
            "evaluated_by": evaluation.evaluated_by,
            "evaluation_date": evaluation.evaluation_date
        }
    
    with open(evaluations_file, 'w') as f:
        json.dump(eval_data, f, indent=2)
    
    print(f"💾 Saved evaluations to {evaluations_file}")
    
    # Compute metrics
    all_metrics = []
    print("\n📊 Computing metrics...")
    print("=" * 50)
    
    for task in tasks:
        unique_task_id = get_unique_task_id(task)
        if unique_task_id in evaluations:
            evaluation = evaluations[unique_task_id]
            metrics = calculator.calculate_all_metrics(task, evaluation)
            all_metrics.append(metrics)
            print(f"\n{format_metrics_report(metrics)}")
    
    # Save metrics
    metrics_file = os.path.join(evaluations_dir, "metrics.json")
    metrics_data = []
    for metrics in all_metrics:
        metrics_data.append({
            "task_id": metrics.task_id,
            "tsr": metrics.tsr,
            "ssr": metrics.ssr,
            "ae": metrics.ae,
            "total_steps": metrics.total_steps,
            "successful_steps": metrics.successful_steps,
            "failed_steps": metrics.failed_steps,
            "actual_actions": metrics.actual_actions,
            "optimal_actions": metrics.optimal_actions,
            "notes": metrics.notes
        })
    
    with open(metrics_file, 'w') as f:
        json.dump(metrics_data, f, indent=2)
    
    print(f"\n💾 Saved metrics to {metrics_file}")
    
    # Show aggregate results
    if all_metrics:
        agg_metrics = aggregate_metrics(all_metrics)
        print(f"\n{format_aggregate_report(agg_metrics)}")
    
    print("\n✅ Evaluation complete!")
    print(f"📁 Results saved in '{evaluations_dir}' directory")


if __name__ == "__main__":
    main()
