python -m cua_bench.rl.off_policy.tinker.loop \
    --base-model Qwen/Qwen3-4B-Instruct-2507 \
    --tasks-path tasks/slack_env \
    --model openai/opencua-7b \
    --max-steps 50 \
    --epochs 5 \
    --lr 1e-5 \
    --gamma 0.99 \
    --checkpoint-every 5 \
    --vllm-url http://localhost:30000/v1

cb run task tasks/slack_env --agent opencua --model openai/opencua-7b --max-steps 50