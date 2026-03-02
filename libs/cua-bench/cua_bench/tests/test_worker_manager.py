"""E2E tests for workers and dataloader.

This module tests the full worker system end-to-end:
- Worker server spawning and management
- Worker client communication
- MultiTurnDataloader with mock model training loop
- ReplayBuffer functionality with tokenization

All tests use real simulated (Playwright) environments - no mocking of env functions.
"""

import time
from pathlib import Path

import pytest

try:
    import torch

    HAS_TORCH = True
except ImportError:
    HAS_TORCH = False

from cua_bench.workers.dataloader import (
    MultiTurnDataloader,
    ReplayBuffer,
)
from cua_bench.workers.worker_client import CBEnvWorkerClient
from cua_bench.workers.worker_manager import (
    WorkerPool,
    cleanup_workers,
    create_workers,
)

# Simple HTML for test tasks
SIMPLE_BUTTON_HTML = """
<div class="flex flex-col items-center justify-center h-full w-full bg-gray-100 p-4">
    <h1 class="text-xl mb-4">Test Task</h1>
    <button
        id="test-btn"
        class="btn bg-blue-500 text-white px-4 py-2 rounded"
        onclick="window.__clicked = true; window.__score = 1.0;"
    >
        Click Me
    </button>
</div>
<script>
    window.__clicked = false;
    window.__score = 0.0;
</script>
"""


class MockTokenizer:
    """Mock tokenizer for testing without HuggingFace dependencies."""

    def __init__(self, vocab_size: int = 1000):
        self.vocab_size = vocab_size
        self.pad_token_id = 0
        self.eos_token_id = 1
        self.bos_token_id = 2

    def encode(self, text: str, add_special_tokens: bool = True) -> list:
        """Simple encoding: convert each character to an ID."""
        ids = [ord(c) % self.vocab_size for c in text]
        if add_special_tokens:
            ids = [self.bos_token_id] + ids + [self.eos_token_id]
        return ids

    def decode(self, ids: list, skip_special_tokens: bool = False) -> str:
        """Simple decoding: convert IDs back to characters."""
        if skip_special_tokens:
            ids = [
                i for i in ids if i not in (self.pad_token_id, self.eos_token_id, self.bos_token_id)
            ]
        return "".join(chr(i) if 32 <= i < 127 else "?" for i in ids)

    def __call__(
        self,
        texts,
        padding=True,
        truncation=True,
        max_length=512,
        return_tensors=None,
        add_special_tokens=True,
    ):
        """Batch tokenization."""
        if isinstance(texts, str):
            texts = [texts]
        encoded = [self.encode(t, add_special_tokens=add_special_tokens) for t in texts]

        # Pad to max length
        max_len = min(max(len(e) for e in encoded), max_length)
        for i in range(len(encoded)):
            if len(encoded[i]) > max_len:
                encoded[i] = encoded[i][:max_len]
            else:
                encoded[i] = encoded[i] + [self.pad_token_id] * (max_len - len(encoded[i]))

        attention_mask = [[1 if t != self.pad_token_id else 0 for t in e] for e in encoded]

        if return_tensors == "pt" and HAS_TORCH:
            return {
                "input_ids": torch.tensor(encoded),
                "attention_mask": torch.tensor(attention_mask),
            }
        return {"input_ids": encoded, "attention_mask": attention_mask}


class TestReplayBuffer:
    """Tests for ReplayBuffer class."""

    def test_buffer_add_and_size(self):
        """Test adding steps to buffer."""
        buffer = ReplayBuffer(capacity=100)

        # Add a complete episode (using tuple format)
        buffer.add(
            (0, {"obs": "obs1", "is_init": True, "done": False, "reward": 0.0}, {"uid": "ep1"})
        )
        buffer.add(
            (0, {"obs": "obs2", "action": "click", "done": False, "reward": 0.0}, {"uid": "ep1"})
        )
        buffer.add(
            (0, {"obs": "obs3", "action": "done", "reward": 1.0, "done": True}, {"uid": "ep1"})
        )

        # Episode is processed on done, all 3 steps should be in buffer
        assert len(buffer) == 3

    def test_buffer_sample(self):
        """Test sampling from buffer."""
        buffer = ReplayBuffer(capacity=100)

        # Add multiple episodes (using tuple format)
        for ep in range(5):
            uid = f"ep{ep}"
            buffer.add(
                (
                    0,
                    {"obs": f"obs{ep}_0", "is_init": True, "done": False, "reward": 0.0},
                    {"uid": uid},
                )
            )
            buffer.add(
                (
                    0,
                    {"obs": f"obs{ep}_1", "action": "click", "reward": 1.0, "done": True},
                    {"uid": uid},
                )
            )

        # Should have 10 steps (2 per episode * 5 episodes)
        assert len(buffer) == 10

        # Sample
        samples = buffer.sample(5)
        assert len(samples) == 5
        assert all(isinstance(s, tuple) and len(s) == 3 for s in samples)

    def test_buffer_sample_insufficient(self):
        """Test sampling when buffer has insufficient data returns what's available."""
        buffer = ReplayBuffer(capacity=100)
        buffer.add(
            (0, {"obs": "obs1", "is_init": True, "done": False, "reward": 0.0}, {"uid": "ep1"})
        )
        buffer.add((0, {"obs": "obs2", "reward": 1.0, "done": True}, {"uid": "ep1"}))

        # Should return what's available
        samples = buffer.sample(10)
        assert len(samples) == 2

    def test_buffer_reward_discounting(self):
        """Test that rewards are discounted backwards."""
        buffer = ReplayBuffer(capacity=100, gamma=0.9)

        # Add episode with reward only at the end (using tuple format)
        buffer.add(
            (0, {"obs": "obs1", "is_init": True, "done": False, "reward": 0.0}, {"uid": "ep1"})
        )
        buffer.add(
            (0, {"obs": "obs2", "action": "a1", "done": False, "reward": 0.0}, {"uid": "ep1"})
        )
        buffer.add(
            (0, {"obs": "obs3", "action": "done", "done": True, "reward": 1.0}, {"uid": "ep1"})
        )

        # Sample all and check rewards are discounted
        samples = buffer.sample(3)
        rewards = sorted([s[1]["reward"] for s in samples], reverse=True)

        # Final step should have reward 1.0
        assert rewards[0] == 1.0
        # Previous step should have 0.9
        assert abs(rewards[1] - 0.9) < 0.01
        # First step should have 0.81
        assert abs(rewards[2] - 0.81) < 0.01

    def test_buffer_only_keep_outcome(self):
        """Test only_keep_outcome mode."""
        buffer = ReplayBuffer(capacity=100, only_keep_outcome=True)

        # Add episode with 3 steps (using tuple format)
        buffer.add(
            (0, {"obs": "obs1", "is_init": True, "done": False, "reward": 0.0}, {"uid": "ep1"})
        )
        buffer.add(
            (0, {"obs": "obs2", "action": "a1", "done": False, "reward": 0.0}, {"uid": "ep1"})
        )
        buffer.add(
            (0, {"obs": "obs3", "action": "done", "reward": 1.0, "done": True}, {"uid": "ep1"})
        )

        # Only final step should be kept
        assert len(buffer) == 1
        sample = buffer.sample(1)[0]
        assert sample[1]["done"] is True

    def test_buffer_clear(self):
        """Test clearing the buffer."""
        buffer = ReplayBuffer(capacity=100)
        buffer.add(
            (0, {"obs": "obs1", "is_init": True, "done": False, "reward": 0.0}, {"uid": "ep1"})
        )
        buffer.add((0, {"obs": "obs2", "reward": 1.0, "done": True}, {"uid": "ep1"}))

        assert len(buffer) == 2
        buffer.clear()
        assert len(buffer) == 0

    def test_buffer_balance_stats(self):
        """Test balance statistics."""
        buffer = ReplayBuffer(capacity=100, balance_thres=0.5)

        # Add episodes with different rewards (using tuple format)
        buffer.add(
            (0, {"obs": "obs1", "is_init": True, "done": False, "reward": 0.0}, {"uid": "ep1"})
        )
        buffer.add((0, {"obs": "obs2", "reward": 0.2, "done": True}, {"uid": "ep1"}))  # below

        buffer.add(
            (1, {"obs": "obs3", "is_init": True, "done": False, "reward": 0.0}, {"uid": "ep2"})
        )
        buffer.add((1, {"obs": "obs4", "reward": 0.8, "done": True}, {"uid": "ep2"}))  # above

        below, above = buffer.get_balance_stats()
        # With gamma=1.0, rewards propagate equally
        # ep1: both steps get 0.2, ep2: both steps get 0.8
        assert below == 2  # All ep1 steps below threshold
        assert above == 2  # All ep2 steps above threshold


class TestWorkerPoolInit:
    """Tests for WorkerPool initialization (no actual spawning)."""

    def test_worker_pool_init(self):
        """Test WorkerPool initialization."""
        pool = WorkerPool(
            n_workers=4,
            allowed_ips=["127.0.0.1"],
            startup_timeout=30.0,
        )

        assert pool.n_workers == 4
        assert pool.allowed_ips == ["127.0.0.1"]
        assert pool.startup_timeout == 30.0


class MockModel:
    """Mock model that returns actions with proper token format."""

    def __init__(self, tokenizer, steps_before_done: int = 2):
        self.tokenizer = tokenizer
        self.steps_before_done = steps_before_done
        self.step_counts = {}

    def generate(self, input_ids, attention_mask=None) -> "torch.Tensor":
        """Generate action response tokens."""
        batch_size = input_ids.shape[0]
        responses = []

        for i in range(batch_size):
            count = self.step_counts.get(i, 0)

            if count >= self.steps_before_done:
                # Return done action
                action_str = "<|action_start|>done()<|action_end|>"
                self.step_counts[i] = 0  # Reset for next episode
            else:
                # Return click action with normalized coordinates
                action_str = "<|action_start|>click(0.5,0.5)<|action_end|>"
                self.step_counts[i] = count + 1

            response_ids = self.tokenizer.encode(action_str, add_special_tokens=False)
            responses.append(response_ids)

        # Pad responses to same length
        max_len = max(len(r) for r in responses)
        padded = []
        for r in responses:
            if len(r) < max_len:
                r = r + [self.tokenizer.pad_token_id] * (max_len - len(r))
            padded.append(r)

        return torch.tensor(padded)


def create_test_task(tmp_path: Path, name: str = "test-task") -> Path:
    """Create a test task directory with simulated provider."""
    task_dir = tmp_path / name
    task_dir.mkdir()
    (task_dir / "main.py").write_text(
        f'''
import cua_bench as cb

HTML = """{SIMPLE_BUTTON_HTML}"""

pid = None

@cb.tasks_config(split="train")
def get_tasks():
    return [cb.Task(
        description="Click the button",
        computer={{"provider": "simulated", "setup_config": {{"width": 800, "height": 600}}}}
    )]

@cb.setup_task(split="train")
async def setup(task, session):
    global pid
    pid = await session.launch_window(html=HTML, title="Test", width=400, height=300)

@cb.evaluate_task(split="train")
async def evaluate(task, session):
    global pid
    clicked = await session.execute_javascript(pid, "window.__clicked")
    return [1.0 if clicked else 0.0]
'''
    )
    return task_dir


class TestWorkerClientE2E:
    """E2E tests for CBEnvWorkerClient with real worker server."""

    @pytest.mark.asyncio
    async def test_client_reset_step_done(self, tmp_path):
        """Test full client flow: reset -> step -> done."""
        task_dir = create_test_task(tmp_path)

        # Start a worker
        workers = await create_workers(
            n_workers=1,
            allowed_ips=["127.0.0.1"],
            startup_timeout=30.0,
        )

        try:
            assert len(workers) == 1
            worker = workers[0]

            # Create task configs
            task_configs = [{"env_path": str(task_dir), "task_index": 0, "split": "train"}]

            # Create client with env_config dict
            env_config = {
                "server_url": worker.api_url,
                "task_configs": task_configs,
                "max_step": 50,
                "max_hist": 10,
                "timeout": 300,
            }
            client = CBEnvWorkerClient(env_config)

            # Reset (picks from task_configs)
            env_ret, meta_info = client.reset()
            assert "obs" in env_ret
            assert env_ret["is_init"] is True
            assert env_ret["done"] is False
            assert "uid" in meta_info

            # Step with click (action is now a string with tags)
            env_ret, meta_info = client.step("<|action_start|>click(500,500)<|action_end|>")
            assert "obs" in env_ret
            assert env_ret["done"] is False

            # Step with done
            env_ret, meta_info = client.step("<|action_start|>done()<|action_end|>")
            assert env_ret["done"] is True
            assert "reward" in env_ret

        finally:
            await cleanup_workers(workers)


class TestDataloaderE2E:
    """E2E tests for MultiTurnDataloader with mock model training loop."""

    @pytest.mark.asyncio
    @pytest.mark.skipif(not HAS_TORCH, reason="torch required")
    async def test_dataloader_training_loop(self, tmp_path):
        """Test dataloader with mock model training loop.

        This simulates the full RL training loop:
            for batch in dataloader:
                responses = model.generate(batch['input_ids'], batch['attention_mask'])
                batch_return = {
                    'prompts': batch['input_ids'],
                    'responses': responses,
                    'attention_mask': combined_mask,
                    'worker_id': batch['worker_id'],
                    'meta_info': batch['meta_info'],
                }
                dataloader.async_step(batch_return)
        """
        task_dir = create_test_task(tmp_path)

        # Start workers
        workers = await create_workers(
            n_workers=1,
            allowed_ips=["127.0.0.1"],
            startup_timeout=30.0,
        )

        try:
            # Create task configs
            task_configs = [{"env_path": str(task_dir), "task_index": 0, "split": "train"}]
            env_configs = [
                {
                    "server_url": w.api_url,
                    "task_configs": task_configs,
                    "max_step": 2,
                    "max_hist": 10,
                    "timeout": 300,
                }
                for w in workers
            ]

            # Create tokenizer
            tokenizer = MockTokenizer()

            # Create dataloader
            dataloader = MultiTurnDataloader(
                env_class=CBEnvWorkerClient,
                env_configs=env_configs,
                tokenizer=tokenizer,
                processor=None,
                is_multi_modal=False,
                batch_size=1,
                replay_capacity=100,
                replay_reward_discount=0.9,
                max_prompt_length=512,
                max_response_length=256,
            )

            # Mock model
            model = MockModel(tokenizer=tokenizer, steps_before_done=2)

            # Run a few iterations of the training loop
            iterations = 0
            max_iterations = 3

            for batch in dataloader:
                if iterations >= max_iterations:
                    break

                # Verify batch structure
                assert "input_ids" in batch
                assert "attention_mask" in batch
                assert "worker_id" in batch
                assert "meta_info" in batch

                # Model generates response tokens
                responses = model.generate(batch["input_ids"], batch["attention_mask"])

                # Create response attention mask
                response_mask = (responses != tokenizer.pad_token_id).long()

                # Build batch_return for async_step
                batch_return = {
                    "prompts": batch["input_ids"],
                    "responses": responses,
                    "attention_mask": torch.cat([batch["attention_mask"], response_mask], dim=1),
                    "worker_id": batch["worker_id"],
                    "meta_info": batch["meta_info"],
                }

                # Step environments
                dataloader.async_step(batch_return)

                iterations += 1

            # Check replay buffer has data
            assert len(dataloader.replay) > 0

            # Check stats
            print(f"\nReplay buffer size: {len(dataloader.replay)}")
            print(f"Running reward: {dataloader.running_outcome_reward():.4f}")
            dataloader.print_stats_in_replay_buffer()

            # Cleanup
            print("Closing dataloader...")
            dataloader.close()
            print("Closing dataloader... DONE")

        finally:
            print("Cleaning up workers")
            await cleanup_workers(workers)
            print("Cleaning up workers... DONE")

    @pytest.mark.asyncio
    @pytest.mark.skipif(not HAS_TORCH, reason="torch required")
    async def test_dataloader_replay_sampling(self, tmp_path):
        """Test sampling from replay buffer after training."""
        task_dir = create_test_task(tmp_path)

        # Start workers
        workers = await create_workers(
            n_workers=1,
            allowed_ips=["127.0.0.1"],
            startup_timeout=30.0,
        )

        try:
            task_configs = [{"env_path": str(task_dir), "task_index": 0, "split": "train"}]
            tokenizer = MockTokenizer()
            env_configs = [
                {
                    "server_url": w.api_url,
                    "task_configs": task_configs,
                    "max_step": 50,
                    "max_hist": 10,
                    "timeout": 300,
                }
                for w in workers
            ]

            dataloader = MultiTurnDataloader(
                env_class=CBEnvWorkerClient,
                env_configs=env_configs,
                tokenizer=tokenizer,
                processor=None,
                is_multi_modal=False,
                batch_size=1,
                replay_capacity=100,
                replay_reward_discount=0.9,
                max_prompt_length=512,
                max_response_length=256,
            )

            model = MockModel(tokenizer=tokenizer, steps_before_done=1)

            # Run enough iterations to fill buffer
            for i, batch in enumerate(dataloader):
                if i >= 5:
                    break
                responses = model.generate(batch["input_ids"], batch["attention_mask"])
                response_mask = (responses != tokenizer.pad_token_id).long()
                batch_return = {
                    "prompts": batch["input_ids"],
                    "responses": responses,
                    "attention_mask": torch.cat([batch["attention_mask"], response_mask], dim=1),
                    "worker_id": batch["worker_id"],
                    "meta_info": batch["meta_info"],
                }
                dataloader.async_step(batch_return)

            # Wait for replay buffer to populate
            time.sleep(1)
            dataloader._gather_env_ret(first_block=False)

            # Sample from buffer
            if len(dataloader.replay) >= 2:
                samples = dataloader.sample_from_buffer(batch_size=2)
                assert samples is not None
                assert "input_ids" in samples
                assert "attention_mask" in samples
                assert "reward" in samples
                print(f"Sampled batch shape: {samples['input_ids'].shape}")

            # Check balance stats
            below, above = dataloader.get_balance_stats()
            print(f"Balance stats - below threshold: {below}, above threshold: {above}")

            dataloader.close()

        finally:
            await cleanup_workers(workers)


class TestDataloaderBenchmark:
    """Benchmark tests for MultiTurnDataloader demonstrating full usage."""

    @pytest.mark.asyncio
    @pytest.mark.skipif(not HAS_TORCH, reason="torch required")
    async def test_dataloader_benchmark(self, tmp_path):
        """Benchmark test demonstrating full dataloader usage.

        This test shows the complete workflow:
        1. Create dataloader with tokenizer/processor
        2. Iterate through batches
        3. Generate mock responses
        4. Step environments
        5. Sample from replay buffer
        6. Get statistics
        """
        task_dir = create_test_task(tmp_path)
        num_workers = 1
        batch_size = 1

        # Start workers
        workers = await create_workers(
            n_workers=num_workers,
            allowed_ips=["127.0.0.1"],
            startup_timeout=30.0,
        )

        try:
            # Create configurations
            task_configs = [{"env_path": str(task_dir), "task_index": 0, "split": "train"}]
            env_configs = [
                {
                    "server_url": w.api_url,
                    "task_configs": task_configs,
                    "max_step": 50,
                    "max_hist": 10,
                    "timeout": 300,
                }
                for w in workers
            ]
            tokenizer = MockTokenizer()

            # Create MultiTurnDataloader
            dataloader = MultiTurnDataloader(
                env_class=CBEnvWorkerClient,
                env_configs=env_configs,
                tokenizer=tokenizer,
                processor=None,
                is_multi_modal=False,
                batch_size=min(batch_size, len(env_configs)),
                replay_capacity=1000,
                replay_reward_discount=0.9,
                max_prompt_length=512,
                max_response_length=256,
                only_keep_outcome_in_replay=False,
            )

            stats = {
                "num_workers": num_workers,
                "batch_size": batch_size,
                "batches_processed": 0,
                "total_steps": 0,
                "replay_buffer_size": 0,
                "running_reward": 0,
                "batch_times": [],
                "errors": [],
            }

            print("\nRunning dataloader iterations...")

            # Process batches from the dataloader
            num_iterations = 5
            for i in range(num_iterations):
                try:
                    start_time = time.time()

                    # Get batch from dataloader (blocks until batch_size envs are ready)
                    batch = next(dataloader)
                    batch_time = time.time() - start_time

                    stats["batches_processed"] += 1
                    stats["batch_times"].append(batch_time)

                    print(
                        f"Batch {i+1}/{num_iterations}: {batch['input_ids'].shape} in {batch_time:.2f}s"
                    )

                    # Simulate model inference - encode action as response tokens
                    # Alternate between click and done to complete episodes
                    batch_size_actual = batch["input_ids"].shape[0]
                    if i % 2 == 0:
                        action_str = "<|action_start|>click(0.5,0.5)<|action_end|>"
                    else:
                        action_str = "<|action_start|>done()<|action_end|>"
                    action_ids = tokenizer.encode(action_str, add_special_tokens=False)
                    action_tensor = torch.tensor(action_ids, dtype=torch.long)

                    # Pad/truncate to fixed length
                    response_len = 64
                    if len(action_ids) < response_len:
                        padded = torch.full(
                            (response_len,), tokenizer.pad_token_id, dtype=torch.long
                        )
                        padded[: len(action_ids)] = action_tensor
                        action_tensor = padded
                    else:
                        action_tensor = action_tensor[:response_len]

                    # Stack for batch
                    responses = action_tensor.unsqueeze(0).repeat(batch_size_actual, 1)
                    response_mask = (responses != tokenizer.pad_token_id).long()

                    # Create batch_return for async_step
                    batch_return = {
                        "prompts": batch["input_ids"],
                        "responses": responses,
                        "attention_mask": torch.cat(
                            [batch["attention_mask"], response_mask], dim=1
                        ),
                        "worker_id": batch["worker_id"],
                        "meta_info": batch["meta_info"],
                    }

                    # Send actions back to environments using async_step
                    dataloader.async_step(batch_return)
                    stats["total_steps"] += batch_size_actual

                    # Give worker time to process and collect results
                    time.sleep(0.2)
                    dataloader._gather_env_ret(first_block=False)

                except Exception as e:
                    print(f"Error in batch {i+1}: {e}")
                    stats["errors"].append(str(e))

            # Give time for replay buffer to populate - need to gather results
            # after done() actions are processed
            for _ in range(5):
                time.sleep(0.5)
                dataloader._gather_env_ret(first_block=False)
                if len(dataloader.replay) > 0:
                    break

            # Get replay buffer stats
            stats["replay_buffer_size"] = len(dataloader.replay)
            stats["running_reward"] = dataloader.running_outcome_reward()

            print("\nReplay Buffer Stats:")
            dataloader.print_stats_in_replay_buffer()

            # Sample from replay buffer
            if len(dataloader.replay) > 0:
                print("Sampling from replay buffer...")
                try:
                    sample_size = min(4, len(dataloader.replay))
                    sample = dataloader.replay.sample(sample_size)
                    print(f"Sampled {len(sample)} experiences from replay buffer")
                    below, above = dataloader.get_balance_stats()
                    print(f"Balance stats - below threshold: {below}, above threshold: {above}")
                except Exception as e:
                    print(f"Could not sample from replay: {e}")

            # Calculate summary
            avg_batch_time = (
                sum(stats["batch_times"]) / len(stats["batch_times"]) if stats["batch_times"] else 0
            )
            throughput = (
                len(stats["batch_times"]) / sum(stats["batch_times"]) if stats["batch_times"] else 0
            )

            print("\n=== MultiTurnDataloader Benchmark Complete ===")
            print(f"Workers: {stats['num_workers']}")
            print(f"Batch size: {stats['batch_size']}")
            print(f"Batches processed: {stats['batches_processed']}")
            print(f"Total steps: {stats['total_steps']}")
            print(f"Replay buffer size: {stats['replay_buffer_size']}")
            print(f"Running reward: {stats['running_reward']:.4f}")
            print(f"Avg batch time: {avg_batch_time:.2f}s")
            print(f"Throughput: {throughput:.2f} batches/sec")

            # Assertions
            assert stats["batches_processed"] == num_iterations
            assert (
                stats["replay_buffer_size"] > 0
            ), "Replay buffer should have data after done() actions"

            dataloader.close()

        finally:
            await cleanup_workers(workers)


class TestWorkerPoolContextManager:
    """E2E tests for WorkerPool as context manager."""

    @pytest.mark.asyncio
    async def test_worker_pool_context_manager(self, tmp_path):
        """Test WorkerPool as async context manager."""
        task_dir = create_test_task(tmp_path)

        async with WorkerPool(
            n_workers=1,
            allowed_ips=["127.0.0.1"],
            startup_timeout=30.0,
        ) as pool:
            assert len(pool.urls) == 1

            # Create client and test
            client = CBEnvWorkerClient(
                {
                    "server_url": pool.urls[0],
                    "task_configs": [
                        {"env_path": str(task_dir), "task_index": 0, "split": "train"}
                    ],
                    "max_step": 50,
                    "max_hist": 10,
                    "timeout": 300,
                }
            )
            env_ret, _ = client.reset()
            assert env_ret["is_init"] is True

        # Workers should be cleaned up after context exit
