"""
Metric calculation functions for evaluating task performance.
"""

from typing import Dict, List, Optional
from dataclasses import dataclass
from utils import Task, Step, get_unique_task_id


@dataclass
class EvaluationResult:
    """Results from human evaluation of a task."""
    task_id: str
    task_successful: bool
    step_failures: List[str]  # List of step IDs that failed
    optimal_step_count: int
    notes: str
    evaluated_by: str
    evaluation_date: str


@dataclass
class TaskMetrics:
    """Computed metrics for a task."""
    task_id: str
    tsr: float  # Task Success Rate (0.0 or 1.0)
    ssr: float  # Step Success Rate (0.0 to 1.0)
    ae: float   # Action Efficiency (0.0 to 1.0)
    total_steps: int
    successful_steps: int
    failed_steps: int
    actual_actions: int
    optimal_actions: int
    notes: str


class MetricsCalculator:
    """Calculator for task performance metrics."""
    
    def calculate_tsr(self, evaluation: EvaluationResult) -> float:
        """
        Calculate Task Success Rate.
        
        TSR = 1.0 if task completed successfully, 0.0 otherwise
        This is based on human evaluation of whether the overall task goal was achieved.
        """
        return 1.0 if evaluation.task_successful else 0.0
    
    def calculate_ssr(self, task: Task, evaluation: EvaluationResult) -> float:
        """
        Calculate Step Success Rate.
        
        SSR = (Total Steps - Failed Steps) / Total Steps
        
        A step is considered failed if:
        1. It has an error in the tool result, OR
        2. It's marked as failed in human evaluation
        """
        if not task.steps:
            return 0.0
        
        total_steps = len(task.steps)
        
        # Count technical failures (steps with errors)
        technical_failures = sum(1 for step in task.steps if not step.is_successful)
        
        # Count human-identified failures
        human_failures = len(evaluation.step_failures)
        
        # Total unique failures (avoid double counting)
        failed_step_ids = set(evaluation.step_failures)
        for step in task.steps:
            if not step.is_successful:
                failed_step_ids.add(step.step_id)
        
        total_failures = len(failed_step_ids)
        successful_steps = total_steps - total_failures
        
        return successful_steps / total_steps
    
    def calculate_ae(self, task: Task, evaluation: EvaluationResult) -> float:
        """
        Calculate Action Efficiency.
        
        AE = min(Optimal Steps, Actual Steps) / max(Optimal Steps, Actual Steps)
        
        This measures how close the agent's plan was to the optimal number of steps.
        Only counts non-screenshot actions as "real work".
        """
        actual_actions = task.action_count  # Exclude screenshots
        optimal_actions = evaluation.optimal_step_count
        
        if optimal_actions == 0 and actual_actions == 0:
            return 1.0  # Perfect efficiency for no-action tasks
        
        if optimal_actions == 0 or actual_actions == 0:
            return 0.0  # No efficiency if one is zero and other isn't
        
        return min(optimal_actions, actual_actions) / max(optimal_actions, actual_actions)
    
    def calculate_all_metrics(self, task: Task, evaluation: EvaluationResult) -> TaskMetrics:
        """Calculate all metrics for a task."""
        tsr = self.calculate_tsr(evaluation)
        ssr = self.calculate_ssr(task, evaluation)
        ae = self.calculate_ae(task, evaluation)
        
        # Count step statistics
        total_steps = len(task.steps)
        failed_step_ids = set(evaluation.step_failures)
        for step in task.steps:
            if not step.is_successful:
                failed_step_ids.add(step.step_id)
        
        failed_steps = len(failed_step_ids)
        successful_steps = total_steps - failed_steps
        
        return TaskMetrics(
            task_id=get_unique_task_id(task),
            tsr=tsr,
            ssr=ssr,
            ae=ae,
            total_steps=total_steps,
            successful_steps=successful_steps,
            failed_steps=failed_steps,
            actual_actions=task.action_count,
            optimal_actions=evaluation.optimal_step_count,
            notes=evaluation.notes
        )


def aggregate_metrics(metrics_list: List[TaskMetrics]) -> Dict[str, float]:
    """
    Aggregate metrics across multiple tasks.
    
    Returns:
        Dictionary with aggregated metrics including:
        - mean_tsr: Average Task Success Rate
        - mean_ssr: Average Step Success Rate  
        - mean_ae: Average Action Efficiency
        - total_tasks: Number of tasks evaluated
        - successful_tasks: Number of tasks that succeeded
    """
    if not metrics_list:
        return {
            "mean_tsr": 0.0,
            "mean_ssr": 0.0,
            "mean_ae": 0.0,
            "total_tasks": 0,
            "successful_tasks": 0
        }
    
    total_tasks = len(metrics_list)
    successful_tasks = sum(1 for m in metrics_list if m.tsr == 1.0)
    
    mean_tsr = sum(m.tsr for m in metrics_list) / total_tasks
    mean_ssr = sum(m.ssr for m in metrics_list) / total_tasks
    mean_ae = sum(m.ae for m in metrics_list) / total_tasks
    
    return {
        "mean_tsr": mean_tsr,
        "mean_ssr": mean_ssr,
        "mean_ae": mean_ae,
        "total_tasks": total_tasks,
        "successful_tasks": successful_tasks
    }


def format_metrics_report(metrics: TaskMetrics) -> str:
    """Format metrics for display."""
    return f"""
Task ID: {metrics.task_id}
Task Success Rate (TSR): {metrics.tsr:.1%}
Step Success Rate (SSR): {metrics.ssr:.1%} ({metrics.successful_steps}/{metrics.total_steps} steps)
Action Efficiency (AE): {metrics.ae:.1%} ({metrics.actual_actions} actual vs {metrics.optimal_actions} optimal)

Details:
- Total Steps: {metrics.total_steps}
- Successful Steps: {metrics.successful_steps}
- Failed Steps: {metrics.failed_steps}
- Actual Actions: {metrics.actual_actions}
- Optimal Actions: {metrics.optimal_actions}

Notes: {metrics.notes}
""".strip()


def format_aggregate_report(agg_metrics: Dict[str, float]) -> str:
    """Format aggregated metrics report."""
    return f"""
=== AGGREGATE METRICS REPORT ===

Tasks Evaluated: {int(agg_metrics['total_tasks'])}
Successful Tasks: {int(agg_metrics['successful_tasks'])}

Average Task Success Rate (TSR): {agg_metrics['mean_tsr']:.1%}
Average Step Success Rate (SSR): {agg_metrics['mean_ssr']:.1%}
Average Action Efficiency (AE): {agg_metrics['mean_ae']:.1%}

Task Success Rate: {agg_metrics['successful_tasks']}/{int(agg_metrics['total_tasks'])} = {agg_metrics['successful_tasks']/max(1, agg_metrics['total_tasks']):.1%}
""".strip()
