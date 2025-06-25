"""
Main evaluation runner with Streamlit interface for human evaluation.
"""

import streamlit as st
import json
import os
from datetime import datetime
from typing import Dict, List, Optional
import pandas as pd
import plotly.express as px
import plotly.graph_objects as go

from utils import LogParser, Task, get_screenshots_for_task, format_task_summary, get_unique_task_id, format_task_display_name
from metrics import MetricsCalculator, EvaluationResult, TaskMetrics, aggregate_metrics, format_metrics_report, format_aggregate_report


class EvaluationStorage:
    """Handles storage and retrieval of evaluation results."""
    
    def __init__(self, storage_dir: str = "evaluations"):
        self.storage_dir = storage_dir
        os.makedirs(storage_dir, exist_ok=True)
        self.evaluations_file = os.path.join(storage_dir, "evaluations.json")
        self.metrics_file = os.path.join(storage_dir, "metrics.json")
    
    def load_evaluations(self) -> Dict[str, EvaluationResult]:
        """Load existing evaluations from storage."""
        if not os.path.exists(self.evaluations_file):
            return {}
        
        try:
            with open(self.evaluations_file, 'r') as f:
                data = json.load(f)
            
            evaluations = {}
            for task_id, eval_data in data.items():
                evaluations[task_id] = EvaluationResult(**eval_data)
            
            return evaluations
        except Exception as e:
            st.error(f"Error loading evaluations: {e}")
            return {}
    
    def save_evaluation(self, evaluation: EvaluationResult):
        """Save a single evaluation result."""
        evaluations = self.load_evaluations()
        evaluations[evaluation.task_id] = evaluation
        
        # Convert to dict for JSON serialization
        data = {}
        for task_id, eval_result in evaluations.items():
            data[task_id] = {
                "task_id": eval_result.task_id,
                "task_successful": eval_result.task_successful,
                "step_failures": eval_result.step_failures,
                "optimal_step_count": eval_result.optimal_step_count,
                "notes": eval_result.notes,
                "evaluated_by": eval_result.evaluated_by,
                "evaluation_date": eval_result.evaluation_date
            }
        
        try:
            with open(self.evaluations_file, 'w') as f:
                json.dump(data, f, indent=2)
        except Exception as e:
            st.error(f"Error saving evaluation: {e}")
    
    def save_metrics(self, metrics_list: List[TaskMetrics]):
        """Save computed metrics."""
        data = []
        for metrics in metrics_list:
            data.append({
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
        
        try:
            with open(self.metrics_file, 'w') as f:
                json.dump(data, f, indent=2)
        except Exception as e:
            st.error(f"Error saving metrics: {e}")


def evaluation_interface():
    """Streamlit interface for human evaluation of tasks."""
    st.title("🔍 Task Evaluation Interface")
    st.write("Human evaluation of Claude Computer Use tasks for computing TSR, SSR, and AE metrics.")
    
    # Initialize components
    logs_dir = "logs"
    storage = EvaluationStorage()
    parser = LogParser(logs_dir)
    calculator = MetricsCalculator()
    
    # Load existing data
    tasks = parser.parse_all_logs()
    evaluations = storage.load_evaluations()
    
    if not tasks:
        st.error("No log files found. Please run some experiments first!")
        return
    
    # Sidebar navigation
    st.sidebar.title("Navigation")
    page = st.sidebar.selectbox("Select Page", [
        "Evaluate Tasks", 
        "View Results", 
        "Metrics Dashboard"
    ])
    
    if page == "Evaluate Tasks":
        evaluate_tasks_page(tasks, evaluations, storage)
    elif page == "View Results":
        view_results_page(tasks, evaluations, calculator)
    elif page == "Metrics Dashboard":
        metrics_dashboard_page(tasks, evaluations, calculator, storage)


def evaluate_tasks_page(tasks: List[Task], evaluations: Dict[str, EvaluationResult], storage: EvaluationStorage):
    """Page for evaluating individual tasks."""
    st.header("📝 Task Evaluation")
    
    # Task selection
    unevaluated_tasks = [t for t in tasks if get_unique_task_id(t) not in evaluations]
    evaluated_tasks = [t for t in tasks if get_unique_task_id(t) in evaluations]
    
    col1, col2 = st.columns(2)
    with col1:
        st.metric("Unevaluated Tasks", len(unevaluated_tasks))
    with col2:
        st.metric("Evaluated Tasks", len(evaluated_tasks))
    
    # Add tabs for different evaluation modes
    tab1, tab2 = st.tabs(["🆕 New Evaluations", "🔄 Re-evaluate Existing"])
    
    with tab1:
        # New evaluations section
        if not unevaluated_tasks:
            st.info("🎉 All tasks have been evaluated! Switch to the 'Re-evaluate Existing' tab to modify existing evaluations.")
        else:
            st.subheader("🔄 Tasks Pending Evaluation")
            _display_task_selection_and_form(unevaluated_tasks, evaluations, storage, is_reevaluation=False)
    
    with tab2:
        # Re-evaluation section
        if not evaluated_tasks:
            st.info("No tasks have been evaluated yet. Switch to the 'New Evaluations' tab to start evaluating tasks.")
        else:
            st.subheader("🔄 Re-evaluate Existing Tasks")
            _display_reevaluation_interface(evaluated_tasks, evaluations, storage)


def _display_task_selection_and_form(task_list: List[Task], evaluations: Dict[str, EvaluationResult], 
                                   storage: EvaluationStorage, is_reevaluation: bool = False):
    """Display task selection dropdown and evaluation form."""
    # Create more descriptive task options with timestamps and session info
    task_options = []
    for t in task_list:
        # Extract timestamp from session_id if available
        session_parts = t.session_id.split('_')
        if len(session_parts) >= 3:
            # Format: testid_category_timestamp
            timestamp_part = session_parts[-1]
            if len(timestamp_part) >= 8:  # YYYYMMDD format
                try:
                    dt = datetime.strptime(timestamp_part[:8], '%Y%m%d')
                    date_str = dt.strftime('%m/%d')
                    time_str = timestamp_part[9:] if len(timestamp_part) > 8 else ""
                    if time_str:
                        formatted_time = f"{time_str[:2]}:{time_str[2:4]}:{time_str[4:6]}"
                        session_info = f"{date_str} {formatted_time}"
                    else:
                        session_info = date_str
                except:
                    session_info = timestamp_part
            else:
                session_info = timestamp_part
        else:
            session_info = t.session_id
        
        # Create descriptive label
        instruction_preview = t.instruction[:40] + "..." if len(t.instruction) > 40 else t.instruction
        task_label = f"[{t.test_id}] {session_info} - {instruction_preview}"
        task_options.append(task_label)
    
    selected_idx = st.selectbox(
        "Select task to evaluate:" if not is_reevaluation else "Select task to re-evaluate:", 
        range(len(task_options)), 
        format_func=lambda x: task_options[x]
    )
    
    selected_task = task_list[selected_idx]
    _display_evaluation_form(selected_task, evaluations, storage)


def _display_reevaluation_interface(evaluated_tasks: List[Task], evaluations: Dict[str, EvaluationResult], 
                                   storage: EvaluationStorage):
    """Display interface for re-evaluating existing tasks."""
    # Initialize session state for re-evaluation tracking
    if 'reevaluating_task_id' not in st.session_state:
        st.session_state.reevaluating_task_id = None
    
    # Create more descriptive task options for evaluated tasks
    eval_task_options = []
    for t in evaluated_tasks:
        # Extract timestamp from session_id if available
        session_parts = t.session_id.split('_')
        if len(session_parts) >= 3:
            # Format: testid_category_timestamp
            timestamp_part = session_parts[-1]
            if len(timestamp_part) >= 8:  # YYYYMMDD format
                try:
                    dt = datetime.strptime(timestamp_part[:8], '%Y%m%d')
                    date_str = dt.strftime('%m/%d')
                    time_str = timestamp_part[9:] if len(timestamp_part) > 8 else ""
                    if time_str:
                        formatted_time = f"{time_str[:2]}:{time_str[2:4]}:{time_str[4:6]}"
                        session_info = f"{date_str} {formatted_time}"
                    else:
                        session_info = date_str
                except:
                    session_info = timestamp_part
            else:
                session_info = timestamp_part
        else:
            session_info = t.session_id
        
        # Create descriptive label
        instruction_preview = t.instruction[:40] + "..." if len(t.instruction) > 40 else t.instruction
        eval_label = f"[{t.test_id}] {session_info} - {instruction_preview}"
        eval_task_options.append(eval_label)
    
    selected_eval_idx = st.selectbox("Select task to re-evaluate:", range(len(eval_task_options)), 
                                   format_func=lambda x: eval_task_options[x])
    
    selected_eval_task = evaluated_tasks[selected_eval_idx]
    unique_task_id = get_unique_task_id(selected_eval_task)
    
    # Check if we're currently re-evaluating this task
    if st.session_state.reevaluating_task_id == unique_task_id:
        # Show the evaluation form for re-evaluation
        st.subheader("🆕 New Evaluation")
        _display_evaluation_form(selected_eval_task, evaluations, storage)
        
        # Add button to cancel re-evaluation
        if st.button("❌ Cancel Re-evaluation"):
            st.session_state.reevaluating_task_id = None
            st.rerun()
            
    else:
        # Show current evaluation details
        if unique_task_id in evaluations:
            current_eval = evaluations[unique_task_id]
            
            st.subheader("📋 Current Evaluation")
            col1, col2, col3 = st.columns(3)
            with col1:
                st.metric("Task Success", "✅ Success" if current_eval.task_successful else "❌ Failed")
            with col2:
                st.metric("Failed Steps", len(current_eval.step_failures))
            with col3:
                st.metric("Optimal Steps", current_eval.optimal_step_count)
            
            if current_eval.notes:
                st.text_area("Current Notes:", value=current_eval.notes, disabled=True, height=100)
            
            st.info(f"📅 Last evaluated by: {current_eval.evaluated_by} on {current_eval.evaluation_date[:10]}")
            
            # Option to start re-evaluation
            col1, col2 = st.columns([1, 3])
            with col1:
                if st.button("🔄 Start Re-evaluation", type="primary"):
                    # Set the re-evaluation flag and remove existing evaluation
                    st.session_state.reevaluating_task_id = unique_task_id
                    if unique_task_id in evaluations:
                        del evaluations[unique_task_id]
                    st.rerun()
            
            with col2:
                st.write("**Note:** This will replace the current evaluation with a new one.")


def _display_evaluation_form(selected_task: Task, evaluations: Dict[str, EvaluationResult], storage: EvaluationStorage):
    """Display the task evaluation form for a given task."""
    # Display task details
    st.subheader("📋 Task Details")
    st.text(format_task_summary(selected_task))
    
    # Show screenshots
    screenshots = get_screenshots_for_task(selected_task)
    if screenshots:
        st.subheader("📸 Screenshots")
        for i, (img_path, timestamp) in enumerate(screenshots):
            with st.expander(f"Screenshot {i+1} - {timestamp}"):
                try:
                    from PIL import Image
                    image = Image.open(img_path)
                    st.image(image, caption=f"Screenshot {i+1}")
                except Exception as e:
                    st.error(f"Could not load image: {e}")
    
    # Show final output
    st.subheader("💬 Final Agent Output")
    st.text_area("Final Output:", selected_task.final_output, height=100, disabled=True)
    
    # Evaluation form
    st.subheader("✅ Human Evaluation")
    with st.form("evaluation_form"):
        col1, col2 = st.columns(2)
        
        with col1:
            task_successful = st.checkbox("Task completed successfully?")
            # Ensure we always have at least 1 as the default value
            default_optimal_steps = max(1, selected_task.action_count if selected_task.action_count > 0 else 1)
            optimal_steps = st.number_input("Optimal number of steps:", min_value=1, value=default_optimal_steps)
        
        with col2:
            evaluator_name = st.text_input("Your name:", value="Evaluator")
        
        # Step failure selection
        st.write("**Select failed steps (if any):**")
        failed_steps = []
        
        if selected_task.steps:
            for i, step in enumerate(selected_task.steps):
                # Create a more readable step description
                step_num = i + 1
                tool_name = step.tool_name
                action = step.tool_input.get("action", "")
                
                # Format different tool types
                if tool_name == "computer":
                    if action == "screenshot":
                        step_desc = f"📸 Screenshot"
                    elif action == "type":
                        text = step.tool_input.get("text", "")
                        step_desc = f"⌨️ Type: '{text}'"
                    elif action == "key":
                        key = step.tool_input.get("text", "")
                        step_desc = f"🔑 Key: {key}"
                    elif action == "click":
                        step_desc = f"🖱️ Click"
                    else:
                        step_desc = f"🖥️ {action}"
                elif tool_name == "bash":
                    cmd = step.tool_input.get("command", "")
                    if len(cmd) > 30:
                        cmd = cmd[:30] + "..."
                    step_desc = f"💻 Bash: {cmd}"
                else:
                    step_desc = f"🔧 {tool_name}"
                
                # Add error indicator if step failed
                if not step.is_successful:
                    step_desc += " ❌ (Error)"
                
                # Add output info if available
                if step.tool_output:
                    output = step.tool_output
                    if len(output) > 20:
                        output = output[:20] + "..."
                    step_desc += f" → {output}"
                
                # Create expandable section for full details
                with st.expander(f"Step {step_num}: {step_desc}", expanded=False):
                    col1, col2 = st.columns([3, 1])
                    
                    with col1:
                        st.write(f"**Tool:** {step.tool_name}")
                        st.write(f"**Input:** {step.tool_input}")
                        if step.tool_output:
                            st.write(f"**Output:** {step.tool_output}")
                        if step.error:
                            st.error(f"**Error:** {step.error}")
                        st.write(f"**Timestamp:** {step.timestamp}")
                    
                    with col2:
                        # Checkbox for marking as failed
                        failed_key = f"failed_step_{step.step_id}"
                        is_failed = st.checkbox("Mark as failed", key=failed_key, 
                                               value=not step.is_successful)
                        if is_failed:
                            failed_steps.append(step.step_id)
        else:
            st.info("No steps found in this task.")
        
        notes = st.text_area("Additional notes:", height=100)
        
        if st.form_submit_button("💾 Save Evaluation"):
            evaluation = EvaluationResult(
                task_id=get_unique_task_id(selected_task),
                task_successful=task_successful,
                step_failures=failed_steps,
                optimal_step_count=optimal_steps,
                notes=notes,
                evaluated_by=evaluator_name,
                evaluation_date=datetime.now().isoformat()
            )
            
            storage.save_evaluation(evaluation)
            
            # Clear re-evaluation state if it was a re-evaluation
            if 'reevaluating_task_id' in st.session_state:
                st.session_state.reevaluating_task_id = None
            
            st.success(f"✅ Evaluation saved for task {selected_task.test_id}")
            st.rerun()


def view_results_page(tasks: List[Task], evaluations: Dict[str, EvaluationResult], calculator: MetricsCalculator):
    """Page for viewing evaluation results."""
    st.header("📊 Evaluation Results")
    
    evaluated_tasks = [t for t in tasks if get_unique_task_id(t) in evaluations]
    
    if not evaluated_tasks:
        st.info("No evaluations available. Please evaluate some tasks first.")
        return
    
    # Compute metrics for all evaluated tasks
    all_metrics = []
    for task in evaluated_tasks:
        unique_task_id = get_unique_task_id(task)
        evaluation = evaluations[unique_task_id]
        metrics = calculator.calculate_all_metrics(task, evaluation)
        all_metrics.append(metrics)
    
    # Show individual results
    st.subheader("📋 Individual Task Results")
    
    # Create a mapping from metrics to tasks for display
    for i, metrics in enumerate(all_metrics):
        corresponding_task = evaluated_tasks[i]  # Tasks and metrics are in same order
        display_name = format_task_display_name(corresponding_task)
        with st.expander(f"{display_name}: TSR={metrics.tsr:.1%}, SSR={metrics.ssr:.1%}, AE={metrics.ae:.1%}"):
            st.text(format_metrics_report(metrics))
    
    # Show aggregate results
    st.subheader("📈 Aggregate Results")
    agg_metrics = aggregate_metrics(all_metrics)
    st.text(format_aggregate_report(agg_metrics))


def metrics_dashboard_page(tasks: List[Task], evaluations: Dict[str, EvaluationResult], 
                          calculator: MetricsCalculator, storage: EvaluationStorage):
    """Dashboard page with visualizations."""
    st.header("📈 Metrics Dashboard")
    
    evaluated_tasks = [t for t in tasks if get_unique_task_id(t) in evaluations]
    
    if not evaluated_tasks:
        st.info("No evaluations available. Please evaluate some tasks first.")
        return
    
    # Compute metrics
    all_metrics = []
    for task in evaluated_tasks:
        unique_task_id = get_unique_task_id(task)
        evaluation = evaluations[unique_task_id]
        metrics = calculator.calculate_all_metrics(task, evaluation)
        all_metrics.append(metrics)
    
    # Save metrics
    storage.save_metrics(all_metrics)
    
    # Create DataFrame for plotting
    df = pd.DataFrame([
        {
            "Task ID": format_task_display_name(evaluated_tasks[i]),
            "Instruction": evaluated_tasks[i].instruction[:100] + "..." if len(evaluated_tasks[i].instruction) > 100 else evaluated_tasks[i].instruction,
            "Full Task ID": m.task_id,  # Keep full ID for internal use
            "TSR": m.tsr,
            "SSR": m.ssr,
            "AE": m.ae,
            "Total Steps": m.total_steps,
            "Successful Steps": m.successful_steps,
            "Failed Steps": m.failed_steps,
            "Actual Actions": m.actual_actions,
            "Optimal Actions": m.optimal_actions
        }
        for i, m in enumerate(all_metrics)
    ])
    
    # Summary metrics
    agg_metrics = aggregate_metrics(all_metrics)
    
    col1, col2, col3, col4 = st.columns(4)
    with col1:
        st.metric("Tasks Evaluated", int(agg_metrics['total_tasks']))
    with col2:
        st.metric("Mean TSR", f"{agg_metrics['mean_tsr']:.1%}")
    with col3:
        st.metric("Mean SSR", f"{agg_metrics['mean_ssr']:.1%}")
    with col4:
        st.metric("Mean AE", f"{agg_metrics['mean_ae']:.1%}")
    
    # Metrics comparison chart
    st.subheader("📊 Metrics Comparison")
    fig_metrics = go.Figure()
    fig_metrics.add_trace(go.Bar(name='TSR', x=df['Task ID'], y=df['TSR']))
    fig_metrics.add_trace(go.Bar(name='SSR', x=df['Task ID'], y=df['SSR']))
    fig_metrics.add_trace(go.Bar(name='AE', x=df['Task ID'], y=df['AE']))
    fig_metrics.update_layout(title="Task Metrics Comparison", barmode='group', yaxis_title="Score")
    st.plotly_chart(fig_metrics, use_container_width=True)
    
    # Group by Task ID analysis
    st.subheader("📊 Results by Task ID")
    
    # Extract test ID from Full Task ID for grouping
    df['Test ID'] = df['Full Task ID'].str.split('_').str[0]
    
    # Group by Test ID and calculate comprehensive statistics
    grouped_metrics = df.groupby('Test ID').agg({
        'TSR': ['mean', 'min', 'max', 'count'],
        'SSR': ['mean', 'min', 'max'],
        'AE': ['mean', 'min', 'max'],
        'Total Steps': ['mean', 'min', 'max'],
        'Actual Actions': ['mean', 'min', 'max'],
        'Optimal Actions': ['mean', 'min', 'max']
    }).round(3)
    
    # Flatten column names
    grouped_metrics.columns = [
        'Mean TSR', 'Min TSR', 'Max TSR', 'Count',
        'Mean SSR', 'Min SSR', 'Max SSR',
        'Mean AE', 'Min AE', 'Max AE',
        'Avg Total Steps', 'Min Total Steps', 'Max Total Steps',
        'Avg Actual Actions', 'Min Actual Actions', 'Max Actual Actions',
        'Avg Optimal Actions', 'Min Optimal Actions', 'Max Optimal Actions'
    ]
    
    # Reset index to make Test ID a column
    grouped_metrics = grouped_metrics.reset_index()
    
    # Display grouped results table
    st.write("**Performance Summary by Task ID (Average, Min, Max):**")
    
    # Create a more readable display with tabs for different metric types
    tab1, tab2, tab3 = st.tabs(["📊 Success Metrics", "🔧 Step Analysis", "📈 Complete View"])
    
    with tab1:
        # Show main success metrics (TSR, SSR, AE)
        success_cols = ['Test ID', 'Count', 'Mean TSR', 'Min TSR', 'Max TSR', 
                       'Mean SSR', 'Min SSR', 'Max SSR', 'Mean AE', 'Min AE', 'Max AE']
        success_df = grouped_metrics.reset_index()[success_cols]
        st.dataframe(success_df.style.format({
            'Mean TSR': '{:.1%}', 'Min TSR': '{:.1%}', 'Max TSR': '{:.1%}',
            'Mean SSR': '{:.1%}', 'Min SSR': '{:.1%}', 'Max SSR': '{:.1%}', 
            'Mean AE': '{:.1%}', 'Min AE': '{:.1%}', 'Max AE': '{:.1%}'
        }), use_container_width=True)
    
    with tab2:
        # Show step-related metrics
        step_cols = ['Test ID', 'Count', 'Avg Total Steps', 'Min Total Steps', 'Max Total Steps',
                    'Avg Actual Actions', 'Min Actual Actions', 'Max Actual Actions',
                    'Avg Optimal Actions', 'Min Optimal Actions', 'Max Optimal Actions']
        step_df = grouped_metrics.reset_index()[step_cols]
        st.dataframe(step_df.style.format({
            'Avg Total Steps': '{:.1f}', 'Min Total Steps': '{:.0f}', 'Max Total Steps': '{:.0f}',
            'Avg Actual Actions': '{:.1f}', 'Min Actual Actions': '{:.0f}', 'Max Actual Actions': '{:.0f}',
            'Avg Optimal Actions': '{:.1f}', 'Min Optimal Actions': '{:.0f}', 'Max Optimal Actions': '{:.0f}'
        }), use_container_width=True)
    
    with tab3:
        # Show all metrics (might be wide, so with horizontal scroll)
        complete_df = grouped_metrics.reset_index()
        format_dict = {
            'Mean TSR': '{:.1%}', 'Min TSR': '{:.1%}', 'Max TSR': '{:.1%}',
            'Mean SSR': '{:.1%}', 'Min SSR': '{:.1%}', 'Max SSR': '{:.1%}', 
            'Mean AE': '{:.1%}', 'Min AE': '{:.1%}', 'Max AE': '{:.1%}',
            'Avg Total Steps': '{:.1f}', 'Min Total Steps': '{:.0f}', 'Max Total Steps': '{:.0f}',
            'Avg Actual Actions': '{:.1f}', 'Min Actual Actions': '{:.0f}', 'Max Actual Actions': '{:.0f}',
            'Avg Optimal Actions': '{:.1f}', 'Min Optimal Actions': '{:.0f}', 'Max Optimal Actions': '{:.0f}'
        }
        st.dataframe(complete_df.style.format(format_dict), use_container_width=True)
    
    # Visualization of grouped metrics
    col1, col2 = st.columns(2)
    
    with col1:
        # Bar chart of average metrics by Task ID with error bars showing min/max
        grouped_reset = grouped_metrics.reset_index()
        fig_grouped = go.Figure()
        
        # Add bars with error bars
        fig_grouped.add_trace(go.Bar(
            name='Mean TSR', 
            x=grouped_reset['Test ID'], 
            y=grouped_reset['Mean TSR'],
            error_y=dict(
                type='data',
                symmetric=False,
                array=grouped_reset['Max TSR'] - grouped_reset['Mean TSR'],
                arrayminus=grouped_reset['Mean TSR'] - grouped_reset['Min TSR']
            )
        ))
        fig_grouped.add_trace(go.Bar(
            name='Mean SSR', 
            x=grouped_reset['Test ID'], 
            y=grouped_reset['Mean SSR'],
            error_y=dict(
                type='data',
                symmetric=False,
                array=grouped_reset['Max SSR'] - grouped_reset['Mean SSR'],
                arrayminus=grouped_reset['Mean SSR'] - grouped_reset['Min SSR']
            )
        ))
        fig_grouped.add_trace(go.Bar(
            name='Mean AE', 
            x=grouped_reset['Test ID'], 
            y=grouped_reset['Mean AE'],
            error_y=dict(
                type='data',
                symmetric=False,
                array=grouped_reset['Max AE'] - grouped_reset['Mean AE'],
                arrayminus=grouped_reset['Mean AE'] - grouped_reset['Min AE']
            )
        ))
        
        fig_grouped.update_layout(
            title="Average Metrics by Task ID (with Min/Max Range)",
            barmode='group',
            xaxis_title="Task ID",
            yaxis_title="Score",
            yaxis=dict(tickformat='.1%')
        )
        st.plotly_chart(fig_grouped, use_container_width=True)
    
    with col2:
        # Task count by Test ID (unchanged)
        fig_count = px.bar(grouped_reset, x='Test ID', y='Count',
                          title="Number of Evaluations by Task ID",
                          labels={'Count': 'Number of Evaluations'})
        st.plotly_chart(fig_count, use_container_width=True)
    
    # Step analysis
    st.subheader("🔧 Step Analysis")
    fig_steps = px.scatter(df, x='Actual Actions', y='Optimal Actions', 
                          size='Total Steps', hover_data=['Task ID'], color='Test ID',
                          title="Actual vs Optimal Actions (Colored by Task ID)")
    fig_steps.add_shape(type="line", x0=0, y0=0, x1=df['Actual Actions'].max(), 
                       y1=df['Actual Actions'].max(), line=dict(dash="dash"))
    st.plotly_chart(fig_steps, use_container_width=True)
    
    # Data table
    st.subheader("📋 Detailed Results")
    
    # Add option to group or show all individual results
    view_option = st.radio("View:", ["All Individual Results", "Grouped by Task ID"], horizontal=True)
    
    if view_option == "All Individual Results":
        # Show all individual results sorted by Test ID
        df_sorted = df.sort_values(['Test ID', 'Task ID'])
        st.dataframe(df_sorted.style.format({
            'TSR': '{:.1%}',
            'SSR': '{:.1%}',
            'AE': '{:.1%}'
        }), use_container_width=True)
    else:
        # Show grouped results with expandable sections for each Task ID
        for test_id in sorted(df['Test ID'].unique()):
            test_df = df[df['Test ID'] == test_id]
            
            # Calculate comprehensive group statistics
            group_stats = {
                'Count': len(test_df),
                'Mean TSR': test_df['TSR'].mean(),
                'Min TSR': test_df['TSR'].min(),
                'Max TSR': test_df['TSR'].max(),
                'Mean SSR': test_df['SSR'].mean(),
                'Min SSR': test_df['SSR'].min(), 
                'Max SSR': test_df['SSR'].max(),
                'Mean AE': test_df['AE'].mean(),
                'Min AE': test_df['AE'].min(),
                'Max AE': test_df['AE'].max(),
                'Avg Steps': test_df['Total Steps'].mean(),
                'Min Steps': test_df['Total Steps'].min(),
                'Max Steps': test_df['Total Steps'].max()
            }
            
            with st.expander(f"**Task ID {test_id}** - {group_stats['Count']} evaluations | "
                           f"TSR: {group_stats['Mean TSR']:.1%} ({group_stats['Min TSR']:.1%}-{group_stats['Max TSR']:.1%}) | "
                           f"SSR: {group_stats['Mean SSR']:.1%} ({group_stats['Min SSR']:.1%}-{group_stats['Max SSR']:.1%}) | "
                           f"AE: {group_stats['Mean AE']:.1%} ({group_stats['Min AE']:.1%}-{group_stats['Max AE']:.1%})"):
                
                # Show comprehensive group summary
                col1, col2, col3, col4 = st.columns(4)
                
                with col1:
                    st.metric("Evaluations", group_stats['Count'])
                    st.write(f"**TSR Range:** {group_stats['Min TSR']:.1%} - {group_stats['Max TSR']:.1%}")
                    
                with col2:
                    st.metric("Avg TSR", f"{group_stats['Mean TSR']:.1%}")
                    st.write(f"**SSR Range:** {group_stats['Min SSR']:.1%} - {group_stats['Max SSR']:.1%}")
                    
                with col3:
                    st.metric("Avg SSR", f"{group_stats['Mean SSR']:.1%}")
                    st.write(f"**AE Range:** {group_stats['Min AE']:.1%} - {group_stats['Max AE']:.1%}")
                    
                with col4:
                    st.metric("Avg AE", f"{group_stats['Mean AE']:.1%}")
                    st.write(f"**Steps Range:** {group_stats['Min Steps']:.0f} - {group_stats['Max Steps']:.0f}")
                
                # Additional statistical summary
                st.write("**📊 Statistical Summary:**")
                summary_col1, summary_col2 = st.columns(2)
                
                with summary_col1:
                    st.write("**Success Rates:**")
                    st.write(f"• TSR: {group_stats['Mean TSR']:.1%} ± {(group_stats['Max TSR'] - group_stats['Min TSR'])/2:.1%}")
                    st.write(f"• SSR: {group_stats['Mean SSR']:.1%} ± {(group_stats['Max SSR'] - group_stats['Min SSR'])/2:.1%}")
                    st.write(f"• AE: {group_stats['Mean AE']:.1%} ± {(group_stats['Max AE'] - group_stats['Min AE'])/2:.1%}")
                
                with summary_col2:
                    st.write("**Step Analysis:**")
                    st.write(f"• Avg Total Steps: {group_stats['Avg Steps']:.1f}")
                    st.write(f"• Avg Actual Actions: {test_df['Actual Actions'].mean():.1f}")
                    st.write(f"• Avg Optimal Actions: {test_df['Optimal Actions'].mean():.1f}")
                
                # Show individual results for this Test ID
                st.write("**📋 Individual Results:**")
                display_cols = ['Task ID', 'Instruction', 'TSR', 'SSR', 'AE', 'Total Steps', 'Actual Actions', 'Optimal Actions']
                st.dataframe(test_df[display_cols].style.format({
                    'TSR': '{:.1%}',
                    'SSR': '{:.1%}',
                    'AE': '{:.1%}'
                }), use_container_width=True)
    
    # Export options
    st.subheader("💾 Export Results")
    col1, col2 = st.columns(2)
    
    with col1:
        if st.button("📄 Download CSV"):
            csv = df.to_csv(index=False)
            st.download_button("Download CSV", csv, "task_metrics.csv", "text/csv")
    
    with col2:
        if st.button("📊 Download JSON"):
            json_data = df.to_json(orient="records", indent=2)
            st.download_button("Download JSON", json_data, "task_metrics.json", "application/json")


if __name__ == "__main__":
    evaluation_interface()
