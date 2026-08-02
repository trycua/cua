#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BENCH_DIR="$(dirname "$SCRIPT_DIR")"
RUNS_DIR="$HOME/.local/share/cua-bench/runs"
EVAL_DIR="$HOME/eval_analysis"
PYTHON="$BENCH_DIR/.venv/bin/python"

BATCHES=(
  "458f5f5c 4bd3e530 4c913e30 55f2eefb 589da233"
  "5dbc9e02 6cb23037 733d278a 83490c3a 84ec333e"
  "854b8a80 9ca3ef44 a6dd38a9 b7148878 b7e51b92"
  "ba34f1c3 c4baee36 d1c655da d5947ae7 ff6dfdde"
)

copy_trajectories() {
  local task_id="$1"
  mkdir -p "$EVAL_DIR/$task_id/trajectories"
  for run_dir in "$RUNS_DIR"/*/; do
    local run_id; run_id=$(basename "$run_dir")
    local src="$run_dir/${task_id}_v0/task_0_agent_logs/trajectories"
    [ -d "$src" ] || continue
    for traj_dir in "$src"/*/; do
      [ -d "$traj_dir" ] || continue
      local traj_name; traj_name=$(basename "$traj_dir")
      local dest="$EVAL_DIR/$task_id/trajectories/${run_id}_${traj_name}"
      [ -d "$dest" ] && continue
      cp -r "$traj_dir" "$dest"
      echo "  copied $task_id: ${run_id}_${traj_name}"
    done
  done
}

batch_num=0
for batch in "${BATCHES[@]}"; do
  batch_num=$((batch_num + 1))
  echo ""
  echo "======================================================"
  echo "BATCH $batch_num: $batch"
  echo "======================================================"

  pids=()
  for task_id in $batch; do
    log="$SCRIPT_DIR/tasks/calibration_run_${task_id}.log"
    echo "  Launching $task_id..."
    "$PYTHON" "$SCRIPT_DIR/run_calibration.py" --tasks "$task_id" --attempts 5 > "$log" 2>&1 &
    pids+=($!)
  done

  echo "  Waiting for PIDs: ${pids[*]}..."
  for pid in "${pids[@]}"; do
    wait "$pid" || echo "  WARNING: PID $pid exited non-zero"
  done
  echo "  Batch $batch_num complete. Copying trajectories..."

  for task_id in $batch; do
    copy_trajectories "$task_id"
  done
  echo "  Done copying batch $batch_num."
done

echo ""
echo "All batches complete."
