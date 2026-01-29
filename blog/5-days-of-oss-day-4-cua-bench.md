# Day 4 of 5 Days of OSS Releases: Cua-Bench

_Published on February 1, 2026 by the Cua Team_

Day 4 — the benchmark we use internally—and with customers—to evaluate CUA agents. Now open-source and MIT-licensed.

## 563 Tasks Across 3 Benchmarks

40 native cua-bench tasks + 369 OSWorld tasks + 154 Windows Agent Arena tasks. One unified harness to evaluate your agent across all of them.


![cuabench_1](https://github.com/user-attachments/assets/ee6de06e-0bc1-40a4-aa1f-79f7da1ebaa5)

```bash
cb run dataset datasets/cua-bench-basic --agent your-agent --model anthropic/claude-sonnet-4-20250514
```

## Parallelized Evaluations

Run tasks across multiple workers simultaneously. Evaluate your agent on 100 tasks in minutes, not hours.


![cuabench_2](https://github.com/user-attachments/assets/3e19d5d6-3721-4ad0-924d-dc5f39aad3a4)

```bash
cb run dataset datasets/cua-bench-basic --max-parallel 8
```

## Works Everywhere

macOS, Windows, Linux, Android. Managed environments you can self-host. Write one agent, evaluate it across every platform.

![cuabench_3](https://github.com/user-attachments/assets/07602652-51cf-4279-851a-11207f5e917c)


## Interactive Exploration

Step through any task manually before running your agent. Understand what you're evaluating.

![cuabench_4](https://github.com/user-attachments/assets/419b7713-8300-422b-bc04-ca6f68791272)


```bash
cb interact tasks/slack_env --variant-id 0
```

## Task Generation

Generate new evaluation tasks from natural language descriptions. Extend the benchmark with your own scenarios.

![cuabench_5](https://github.com/user-attachments/assets/5f53a0b3-313a-4029-a5f5-abf1e77c9c3c)


```bash
cb task generate "2048 game"
```

## Live Monitoring

Watch your agent's progress in real-time. View traces, logs, and screenshots as evaluations run.

![cuabench_6](https://github.com/user-attachments/assets/97af5e09-bc88-48fb-9e82-d23bb6f1a88f)


```bash
cb run watch <run_id>
cb trace view <run_id>
```

## Oracle Validation

Run tasks with reference implementations to verify environments work correctly before agent evaluation.

![cuabench_7](https://github.com/user-attachments/assets/6689cdeb-843e-45db-8729-a1e80fb60b98)


```bash
cb run dataset datasets/cua-bench-basic --oracle
```

## Adapters for Existing Benchmarks

Plug in OSWorld, Windows Agent Arena, AndroidWorld. Unified CLI, consistent evaluation, comparable results.

![cuabench_8](https://github.com/user-attachments/assets/77b0b545-42e1-480b-92d9-22380c8c64ec)


---

![cuabench_9](https://github.com/user-attachments/assets/09ad87ec-0a54-401a-bb33-93e891df5df2)


**MIT licensed. Self-hostable. 563 tasks and counting.**

- [GitHub Repository](https://github.com/trycua/cua)
