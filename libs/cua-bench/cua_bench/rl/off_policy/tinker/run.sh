python -m cua_bench.rl.off_policy.tinker.loop \
    --base-model xlang/OpenCUA-7B \
    --tasks-path tasks/slack_env \
    --model openai/opencua-7b \
    --max-steps 50 \
    --epochs 20 \
    --lr 1e-5 \
    --gamma 0.99 \
    --checkpoint-every 5 \
    --vllm-url http://localhost:30000/v1