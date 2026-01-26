"""Dataloader for RL training with parallel environment workers.

This module provides:
- ReplayBuffer: Episode-aware replay buffer with reward discounting for RL
- MultiTurnDataloader: Handles parallel environment rollout, batching, and preprocessing

The dataloader is designed to be compatible with RL training frameworks like TRL's
GRPOTrainer, supporting tokenized inputs/outputs with proper attention masks.

Example:
    from transformers import AutoTokenizer
    from cua_bench.workers import MultiTurnDataloader, CBEnvWorkerClient

    tokenizer = AutoTokenizer.from_pretrained("Qwen/Qwen2-0.5B-Instruct")

    dataloader = MultiTurnDataloader(
        env_class=CBEnvWorkerClient,
        env_configs=[{"server_url": f"http://localhost:{8001+i}"} for i in range(4)],
        tokenizer=tokenizer,
        processor=None,
        is_multi_modal=False,
        batch_size=4,
        replay_capacity=1000,
        replay_reward_discount=0.9,
        max_prompt_length=512,
        max_response_length=256,
    )

    for batch in dataloader:
        # batch contains: input_ids, attention_mask, worker_id, meta_info
        responses = model.generate(batch['input_ids'], batch['attention_mask'])

        batch_return = {
            'prompts': batch['input_ids'],
            'responses': responses,
            'attention_mask': torch.cat([batch['attention_mask'], response_mask], dim=1),
            'worker_id': batch['worker_id'],
            'meta_info': batch['meta_info'],
        }
        dataloader.async_step(batch_return)

    # Sample from replay buffer for training
    replay_batch = dataloader.sample_from_buffer(batch_size=32)
"""

import base64
import io
import multiprocessing as mp
import queue
import random
import re
from typing import Any, Dict, Iterator, List, Optional, Tuple, Type

import numpy as np

try:
    import torch

    HAS_TORCH = True
except ImportError:
    HAS_TORCH = False

try:
    from PIL import Image

    HAS_PIL = True
except ImportError:
    HAS_PIL = False

from .worker_client import CBEnvWorkerClient


def collate_fn(data_list: List[Dict]) -> Dict:
    """Collate a list of data dicts into a batched dict.

    Handles both tensor and non-tensor data appropriately:
    - Tensors are stacked along dimension 0
    - Non-tensors are collected into numpy arrays

    Args:
        data_list: List of dictionaries with tensor/non-tensor values

    Returns:
        Batched dictionary with stacked tensors and collected non-tensors
    """
    if not HAS_TORCH:
        raise ImportError("torch is required for collate_fn")

    tensors = {}
    non_tensors = {}

    for data in data_list:
        for key, val in data.items():
            if isinstance(val, torch.Tensor):
                if key not in tensors:
                    tensors[key] = []
                tensors[key].append(val)
            else:
                if key not in non_tensors:
                    non_tensors[key] = []
                non_tensors[key].append(val)

    for key, val in tensors.items():
        if key in ["responses", "input_ids", "attention_mask", "position_ids"]:
            tensors[key] = torch.stack(val, dim=0).to(torch.int64)
        else:
            tensors[key] = torch.stack(val, dim=0)

    for key, val in non_tensors.items():
        non_tensors[key] = np.array(val, dtype=object)

    output = {}
    output.update(tensors)
    output.update(non_tensors)
    return output


def process_image(image_b64: str) -> "Image.Image":
    """Process a base64-encoded image string into a PIL Image.

    Args:
        image_b64: Base64-encoded image bytes

    Returns:
        PIL Image in RGB format
    """
    if not HAS_PIL:
        raise ImportError("PIL is required for process_image")

    jpg_bytes = base64.b64decode(image_b64)
    buffer = io.BytesIO(jpg_bytes)
    image = Image.open(buffer)

    if image.mode != "RGB":
        image = image.convert("RGB")

    return image


def replace_vision_tags(input_string: str) -> Tuple[str, List[str]]:
    """Replace vision tags with <image> placeholder and extract image strings.

    Args:
        input_string: String with <|vision_start|>...<|vision_end|> tags

    Returns:
        Tuple of (modified string with <image> placeholders, list of extracted image strings)
    """
    extracted_strings = []
    pattern = r"<\|vision_start\|>(.*?)<\|vision_end\|>"

    def replacement(match):
        extracted_strings.append(match.group(1))
        return "<image>"

    result = re.sub(pattern, replacement, input_string, flags=re.DOTALL)
    return result, extracted_strings


def tokenize_and_postprocess(
    prompt: str,
    tokenizer: Any,
    max_length: int,
    pad_token_id: int,
    left_pad: bool = True,
    truncation: str = "left",
) -> Tuple["torch.Tensor", "torch.Tensor"]:
    """Tokenize and pad/truncate a prompt string.

    Args:
        prompt: Text to tokenize
        tokenizer: HuggingFace tokenizer
        max_length: Maximum sequence length
        pad_token_id: Token ID to use for padding
        left_pad: If True, pad on the left side
        truncation: 'left' or 'right' truncation

    Returns:
        Tuple of (input_ids, attention_mask) tensors of shape (1, max_length)
    """
    if not HAS_TORCH:
        raise ImportError("torch is required for tokenize_and_postprocess")

    # Tokenize without padding
    encoded = tokenizer.encode(prompt, add_special_tokens=False)

    # Truncate if needed
    if len(encoded) > max_length:
        if truncation == "left":
            encoded = encoded[-max_length:]
        else:
            encoded = encoded[:max_length]

    # Create attention mask (1 for real tokens, 0 for padding)
    attention_mask = [1] * len(encoded)

    # Pad if needed
    pad_length = max_length - len(encoded)
    if pad_length > 0:
        if left_pad:
            encoded = [pad_token_id] * pad_length + encoded
            attention_mask = [0] * pad_length + attention_mask
        else:
            encoded = encoded + [pad_token_id] * pad_length
            attention_mask = attention_mask + [0] * pad_length

    input_ids = torch.tensor([encoded], dtype=torch.long)
    attention_mask = torch.tensor([attention_mask], dtype=torch.long)

    return input_ids, attention_mask


def compute_position_ids(attention_mask: "torch.Tensor") -> "torch.Tensor":
    """Compute position IDs from attention mask.

    Args:
        attention_mask: Attention mask tensor of shape (batch, seq_len)

    Returns:
        Position IDs tensor of shape (batch, seq_len)
    """
    if not HAS_TORCH:
        raise ImportError("torch is required for compute_position_ids")

    # Position IDs are cumulative sum of attention mask minus 1
    # This gives 0, 1, 2, ... for real tokens and continues from last position for padding
    position_ids = attention_mask.cumsum(dim=-1) - 1
    position_ids = position_ids.clamp(min=0)
    return position_ids


class ReplayBuffer:
    """Replay buffer with episode-aware reward discounting for RL training.

    This buffer stores experience tuples and supports:
    - Episode-based organization (by uid)
    - Reward discounting on episode completion
    - Configurable capacity with FIFO eviction
    - Optional filtering to only keep final outcomes
    - Balance statistics for reward distribution analysis

    The buffer stores tuples of (worker_id, env_ret, meta_info) where:
    - worker_id: ID of the worker that produced this step
    - env_ret: Dictionary with obs, action, reward, done, etc.
    - meta_info: Dictionary with uid and other metadata

    Attributes:
        capacity: Maximum number of steps to store
        gamma: Discount factor for reward propagation
        only_keep_outcome: If True, only keep final steps with outcome
        balance_thres: Threshold for balance statistics
    """

    def __init__(
        self,
        capacity: int = 10000,
        gamma: float = 1.0,
        only_keep_outcome: bool = False,
        balance_thres: float = 0.1,
    ):
        """Initialize the replay buffer.

        Args:
            capacity: Maximum capacity of the replay buffer
            gamma: Discount factor for reward calculation
            only_keep_outcome: Whether to only keep the final step of episodes
            balance_thres: Threshold for balance statistics
        """
        self.capacity = capacity
        self.gamma = gamma
        self.only_keep_outcome = only_keep_outcome
        self.balance_thres = balance_thres

        # Main buffer for ready-to-sample experiences
        self.ready_buffer: List[Optional[Tuple]] = [None] * capacity
        self.ready_position = 0
        self.ready_count = 0

        # Temporary storage for ongoing episodes
        self.episode_buffer: Dict[str, List[Tuple]] = {}

    def add(
        self,
        worker_id: int,
        env_ret: Dict[str, Any],
        meta_info: Dict[str, Any],
    ) -> None:
        """Add data to the replay buffer.

        Args:
            worker_id: ID of the worker that produced this step
            env_ret: Environment return dict with obs, action, reward, done, etc.
            meta_info: Meta info dict with uid
        """
        data = (worker_id, env_ret, meta_info)
        uid = meta_info.get("uid", "unknown")
        done = env_ret.get("done", False)

        # If this is the first step of an episode, initialize the episode buffer
        if uid not in self.episode_buffer:
            self.episode_buffer[uid] = []

        # Add the current step to the episode buffer
        self.episode_buffer[uid].append(data)

        # If episode is done, process it and add to ready buffer
        if done:
            self._process_episode(uid)

    def _process_episode(self, uid: str) -> None:
        """Process a completed episode with discounted rewards.

        Args:
            uid: Unique ID of the episode
        """
        if uid not in self.episode_buffer:
            return

        episode = self.episode_buffer[uid]

        # Calculate discounted returns for each step (G = r + gamma * G_next)
        rewards = [step[1].get("reward", 0.0) for step in episode]

        # Calculate discounted returns (working backwards)
        discounted_returns = [0.0] * len(rewards)
        discounted_returns[-1] = rewards[-1]

        for i in range(len(rewards) - 2, -1, -1):
            discounted_returns[i] = rewards[i] + self.gamma * discounted_returns[i + 1]

        # If only keeping outcome, just store the final step
        if self.only_keep_outcome:
            self._add_to_ready_buffer(episode[-1])
        else:
            # Add all steps with properly calculated returns
            for i in range(len(episode)):
                worker_id, env_ret, meta_info = episode[i]
                env_ret = env_ret.copy()
                env_ret["reward"] = discounted_returns[i]
                self._add_to_ready_buffer((worker_id, env_ret, meta_info))

        # Clear the episode from temporary storage
        del self.episode_buffer[uid]

    def _add_to_ready_buffer(self, data: Tuple) -> None:
        """Add a processed experience to the ready buffer.

        Args:
            data: Processed experience tuple (worker_id, env_ret, meta_info)
        """
        self.ready_buffer[self.ready_position] = data
        self.ready_position = (self.ready_position + 1) % self.capacity
        self.ready_count = min(self.ready_count + 1, self.capacity)

    def get_balance_stats(self) -> Tuple[int, int]:
        """Get balance statistics for reward distribution.

        Returns:
            Tuple of (below_threshold_count, above_threshold_count)
        """
        below = 0
        above = 0
        for data in self.ready_buffer:
            if data is None:
                continue
            _, env_ret, _ = data
            reward = env_ret.get("reward", 0.0)
            if reward < self.balance_thres:
                below += 1
            else:
                above += 1
        return below, above

    def should_keep(self, curr_below: int, curr_above: int, curr_ret: float) -> bool:
        """Determine if a sample should be kept based on balance.

        Args:
            curr_below: Current count below threshold
            curr_above: Current count above threshold
            curr_ret: Current return value

        Returns:
            True if sample should be kept
        """
        rate = (curr_above - curr_below) / (curr_above + curr_below + 1e-8)
        keep = False
        if -0.1 < rate < 0.1:
            keep = True
        elif rate >= 0.1:
            keep = True if curr_ret < self.balance_thres else False
        else:
            keep = True if curr_ret > self.balance_thres else False
        return keep

    def sample(self, batch_size: int) -> List[Tuple]:
        """Sample experiences from the ready buffer.

        Args:
            batch_size: Number of experiences to sample

        Returns:
            List of sampled experience tuples

        Raises:
            ValueError: If buffer has fewer valid samples than requested
        """
        if self.ready_count == 0:
            return []

        valid_indices = [i for i in range(self.ready_count) if self.ready_buffer[i] is not None]

        if len(valid_indices) < batch_size:
            sampled_indices = random.sample(valid_indices, len(valid_indices))
        else:
            sampled_indices = random.sample(valid_indices, batch_size)

        return [self.ready_buffer[i] for i in sampled_indices]

    def clear(self) -> None:
        """Clear both ready buffer and episode buffer."""
        self.ready_buffer = [None] * self.capacity
        self.ready_position = 0
        self.ready_count = 0
        self.episode_buffer = {}

    def __len__(self) -> int:
        """Return the number of valid experiences in the ready buffer."""
        return sum(1 for item in self.ready_buffer if item is not None)


def _env_worker_process(
    env_config: Dict[str, Any],
    recv_queue: mp.Queue,
    send_queue: mp.Queue,
    worker_id: int,
    env_class: Type,
    task_configs: List[Dict[str, Any]],
) -> None:
    """Worker process that runs an environment.

    This function manages the environment lifecycle:
    - Initializes the environment from config
    - Resets with a random task from task_configs
    - Steps based on received actions
    - Auto-resets on episode completion

    Args:
        env_config: Configuration dict for the environment client
        recv_queue: Queue for receiving (worker_id, action, meta_info) tuples
        send_queue: Queue for sending (worker_id, env_ret, meta_info) tuples
        worker_id: ID of this worker
        env_class: Environment client class (e.g., CBEnvWorkerClient)
        task_configs: List of task configurations to sample from
    """
    # Create environment client
    env = env_class(**env_config)

    # Initial reset with random task
    task_config = random.choice(task_configs)
    env_path = task_config.get("env_path", "")
    task_index = task_config.get("task_index", 0)
    split = task_config.get("split", "train")

    try:
        env_ret, meta_info = env.reset(env_path, task_index, split)
        send_queue.put((worker_id, env_ret, meta_info))
    except Exception as e:
        send_queue.put((worker_id, {"error": str(e), "is_init": True, "done": False}, {}))

    while True:
        try:
            item = recv_queue.get(timeout=1.0)
        except queue.Empty:
            continue

        wid, action, _ = item
        if wid is None and action is None:
            # Shutdown signal
            break

        assert wid == worker_id, f"Worker {worker_id} received action for worker {wid}"

        try:
            env_ret, meta_info = env.step(action)
            send_queue.put((worker_id, env_ret, meta_info))

            if env_ret.get("done", False):
                # Auto-reset: wait for signal then reset
                try:
                    item = recv_queue.get(timeout=30.0)
                except queue.Empty:
                    continue

                # Reset with new random task
                task_config = random.choice(task_configs)
                env_path = task_config.get("env_path", "")
                task_index = task_config.get("task_index", 0)
                split = task_config.get("split", "train")

                env_ret, meta_info = env.reset(env_path, task_index, split)
                send_queue.put((worker_id, env_ret, meta_info))

        except Exception as e:
            send_queue.put((worker_id, {"error": str(e), "done": False, "is_init": False}, {}))


class MultiTurnDataloader:
    """Dataloader for RL training with parallel environment workers.

    This dataloader handles:
    - Parallel environment management via multiprocessing
    - Tokenization and batching of observations
    - Action dispatch from model responses
    - Replay buffer management for experience storage

    The dataloader yields batches suitable for model inference:
    - input_ids: Tokenized observations (batch, seq_len)
    - attention_mask: Attention mask (batch, seq_len)
    - position_ids: Position IDs (batch, seq_len)
    - worker_id: Worker IDs for each sample
    - meta_info: Metadata for each sample

    Example:
        dataloader = MultiTurnDataloader(
            env_class=CBEnvWorkerClient,
            env_configs=[{"server_url": f"http://localhost:{8001+i}"} for i in range(4)],
            tokenizer=tokenizer,
            processor=processor,
            is_multi_modal=True,
            batch_size=4,
            replay_capacity=1000,
            replay_reward_discount=0.9,
            max_prompt_length=512,
            max_response_length=256,
        )

        for batch in dataloader:
            responses = model.generate(batch['input_ids'])
            dataloader.async_step({
                'prompts': batch['input_ids'],
                'responses': responses,
                'attention_mask': combined_mask,
                'worker_id': batch['worker_id'],
                'meta_info': batch['meta_info'],
            })

    Attributes:
        num_envs: Number of parallel environments
        batch_size: Batch size for inference
        replay: Replay buffer for experience storage
    """

    def __init__(
        self,
        env_class: Type,
        env_configs: List[Dict[str, Any]],
        tokenizer: Any,
        processor: Optional[Any] = None,
        is_multi_modal: bool = False,
        batch_size: int = 8,
        replay_capacity: int = 10000,
        replay_reward_discount: float = 0.9,
        max_prompt_length: int = 1024,
        max_response_length: int = 1024,
        only_keep_outcome_in_replay: bool = False,
        task_configs: Optional[List[Dict[str, Any]]] = None,
    ):
        """Initialize the dataloader.

        Args:
            env_class: Environment client class (e.g., CBEnvWorkerClient)
            env_configs: List of configuration dicts for each environment.
                Each config should have at minimum {"server_url": "http://..."}
            tokenizer: HuggingFace tokenizer for text processing
            processor: Optional HuggingFace processor for multimodal inputs
            is_multi_modal: Whether to process multimodal (image) inputs
            batch_size: Batch size for inference (must be <= num_envs)
            replay_capacity: Capacity of replay buffer
            replay_reward_discount: Discount factor (gamma) for reward propagation
            max_prompt_length: Maximum prompt length for tokenization
            max_response_length: Maximum response length for tokenization
            only_keep_outcome_in_replay: If True, only keep final steps in replay
            task_configs: List of task configurations to sample from.
                Each config should have {"env_path": "...", "task_index": 0, "split": "train"}
        """
        if not HAS_TORCH:
            raise ImportError("torch is required for MultiTurnDataloader")

        self.num_envs = len(env_configs)
        self._tokenizer = tokenizer
        self._processor = processor
        self._is_multi_modal = is_multi_modal
        self.batch_size = batch_size

        assert (
            self.batch_size <= self.num_envs
        ), f"batch_size ({batch_size}) must be <= num_envs ({self.num_envs})"

        # Initialize replay buffer
        self.replay = ReplayBuffer(
            capacity=replay_capacity,
            gamma=replay_reward_discount,
            only_keep_outcome=only_keep_outcome_in_replay,
        )
        self._only_keep_outcome_in_replay = only_keep_outcome_in_replay

        # Task configs (default to empty task if not provided)
        if task_configs is None:
            task_configs = [{"env_path": "", "task_index": 0, "split": "train"}]
        self._task_configs = task_configs

        # Create queues for each worker
        self._action_queues: List[mp.Queue] = [mp.Queue() for _ in range(self.num_envs)]
        self._env_ret_queue: mp.Queue = mp.Queue()

        # State tracking
        self._env_states: List[List[Tuple]] = [[] for _ in range(self.num_envs)]
        self._env_global_steps: List[int] = [0 for _ in range(self.num_envs)]
        self._max_prompt_length = max_prompt_length
        self._max_response_length = max_response_length
        self._running_reward = 0.0

        # Start worker processes
        self._workers: List[mp.Process] = []
        for wid in range(self.num_envs):
            w = mp.Process(
                target=_env_worker_process,
                args=(
                    env_configs[wid],
                    self._action_queues[wid],
                    self._env_ret_queue,
                    wid,
                    env_class,
                    task_configs,
                ),
            )
            w.daemon = True
            w.start()
            self._workers.append(w)

        self._running = True

    def __iter__(self) -> Iterator[Dict[str, Any]]:
        return self

    def __next__(self) -> Dict[str, Any]:
        """Get the next batch of observations.

        This method:
        1. Gathers environment returns from workers
        2. Waits until batch_size environments are ready
        3. Preprocesses and tokenizes the observations
        4. Returns a batched dict suitable for model inference

        Returns:
            Dict with:
            - input_ids: (batch, seq_len) tokenized observations
            - attention_mask: (batch, seq_len) attention mask
            - position_ids: (batch, seq_len) position IDs
            - worker_id: numpy array of worker IDs
            - meta_info: numpy array of meta info dicts
            - multi_modal_inputs (optional): dict of image tensors if multimodal
        """
        self._gather_env_ret(first_block=False)
        ready_envs = [len(self._env_states[i]) > 0 for i in range(self.num_envs)]

        while sum(ready_envs) < self.batch_size:
            self._gather_env_ret(first_block=True)
            ready_envs = [len(self._env_states[i]) > 0 for i in range(self.num_envs)]

        # Select batch_size ready environments
        ids = self._make_indices(ready_envs, self.batch_size)
        batch = [self._env_states[i].pop(0) for i in ids]

        # Preprocess and tokenize
        batch = self._preprocess_batch(batch, include_response=False, use_prev_obs=False)
        batch = collate_fn(batch)

        return batch

    def _preprocess_batch(
        self,
        batch: List[Tuple],
        include_response: bool = True,
        use_prev_obs: bool = True,
    ) -> List[Dict]:
        """Preprocess a batch of (worker_id, state, meta_info) tuples.

        Args:
            batch: List of (worker_id, env_ret, meta_info) tuples
            include_response: Whether to include action/response tokens
            use_prev_obs: Whether to use prev_obs instead of obs

        Returns:
            List of preprocessed dicts ready for collation
        """
        rows = []
        for wid, state, meta_info in batch:
            row_dict = {}
            prompt = state.get("prev_obs") if use_prev_obs else state.get("obs", "")
            if prompt is None:
                prompt = state.get("obs", "")

            raw_prompt = prompt

            if self._is_multi_modal and self._processor is not None:
                prompt, jpg_strs = replace_vision_tags(prompt)
                raw_prompt = prompt.replace(
                    "<image>", "<|vision_start|><|image_pad|><|vision_end|>"
                )

                if jpg_strs:
                    row_dict["multi_modal_data"] = {
                        "image": [process_image(img) for img in jpg_strs]
                    }
                    image_inputs = self._processor.image_processor(
                        row_dict["multi_modal_data"]["image"], return_tensors="pt"
                    )
                    image_grid_thw = image_inputs.get("image_grid_thw")
                    row_dict["multi_modal_inputs"] = {k: v for k, v in image_inputs.items()}

                    if image_grid_thw is not None:
                        merge_length = self._processor.image_processor.merge_size**2
                        index = 0
                        while "<image>" in prompt:
                            prompt = prompt.replace(
                                "<image>",
                                "<|vision_start|>"
                                + "<|placeholder|>" * (image_grid_thw[index].prod() // merge_length)
                                + "<|vision_end|>",
                                1,
                            )
                            index += 1
                        prompt = prompt.replace("<|placeholder|>", self._processor.image_token)

            # Tokenize prompt
            pad_token_id = self._tokenizer.pad_token_id
            if pad_token_id is None:
                pad_token_id = self._tokenizer.eos_token_id

            input_ids, attention_mask = tokenize_and_postprocess(
                prompt=prompt,
                tokenizer=self._tokenizer,
                max_length=self._max_prompt_length,
                pad_token_id=pad_token_id,
                left_pad=True,
                truncation="left",
            )

            if include_response:
                action_str = state.get("action", "")
                response_ids, response_attention_mask = tokenize_and_postprocess(
                    prompt=action_str,
                    tokenizer=self._tokenizer,
                    max_length=self._max_response_length,
                    pad_token_id=pad_token_id,
                    left_pad=False,
                    truncation="right",
                )
                input_ids = torch.cat([input_ids, response_ids], dim=1)
                attention_mask = torch.cat([attention_mask, response_attention_mask], dim=1)
                row_dict["responses"] = response_ids[0]
                row_dict["reward"] = torch.tensor(state.get("reward", 0.0)).float()

            # Compute position IDs
            position_ids = compute_position_ids(attention_mask)

            row_dict["input_ids"] = input_ids[0]
            row_dict["attention_mask"] = attention_mask[0]
            row_dict["position_ids"] = position_ids[0]
            row_dict["worker_id"] = wid
            row_dict["meta_info"] = meta_info
            row_dict["raw_prompt_ids"] = self._tokenizer.encode(
                raw_prompt, add_special_tokens=False
            )

            rows.append(row_dict)
        return rows

    def _process_batch_return(
        self,
        batch_return: Dict[str, Any],
    ) -> Tuple[List[Tuple[int, str]], List[Dict]]:
        """Process batch return from model to extract actions.

        This method decodes response tokens to extract action strings
        in the format <|action_start|>action_str<|action_end|>.

        Args:
            batch_return: Dict with:
                - prompts: (batch, prompt_len) prompt token IDs
                - responses: (batch, response_len) response token IDs
                - attention_mask: (batch, total_len) combined attention mask
                - worker_id: numpy array of worker IDs
                - meta_info: numpy array of meta info dicts

        Returns:
            Tuple of (actions, meta_infos) where:
            - actions: List of (worker_id, action_string) tuples
            - meta_infos: List of meta info dicts
        """
        actions = []
        meta_infos = []

        for i in range(len(batch_return["prompts"])):
            prompt_length = batch_return["prompts"][i].shape[-1]

            response_ids = batch_return["responses"][i]
            valid_response_length = batch_return["attention_mask"][i][prompt_length:].sum()
            valid_response_ids = response_ids[:valid_response_length]
            sequences_str = self._tokenizer.decode(valid_response_ids, skip_special_tokens=False)

            # Extract action from the sequence
            action_str = self._extract_action_from_response(sequences_str)

            actions.append((batch_return["worker_id"][i], action_str))
            meta_infos.append(batch_return["meta_info"][i])

        return actions, meta_infos

    def _extract_action_from_response(self, response_str: str) -> Dict[str, Any]:
        """Extract action dict from model response string.

        Parses action strings like:
        - <|action_start|>click(0.5,0.5)<|action_end|>
        - <|action_start|>type("hello")<|action_end|>
        - <|action_start|>done()<|action_end|>

        Args:
            response_str: Full response string from model

        Returns:
            Action dictionary suitable for environment step
        """
        # Extract content between action tags
        action_match = re.search(
            r"<\|action_start\|>(.*?)<\|action_end\|>", response_str, re.DOTALL
        )

        if not action_match:
            # Default to done if no action found
            return {"type": "DoneAction"}

        action_str = action_match.group(1).strip()

        # Parse different action types
        # click(x, y)
        click_match = re.match(r"click\s*\(\s*([\d.]+)\s*,\s*([\d.]+)\s*\)", action_str)
        if click_match:
            return {
                "type": "ClickAction",
                "x": float(click_match.group(1)),
                "y": float(click_match.group(2)),
            }

        # right_click(x, y)
        right_click_match = re.match(r"right_click\s*\(\s*([\d.]+)\s*,\s*([\d.]+)\s*\)", action_str)
        if right_click_match:
            return {
                "type": "RightClickAction",
                "x": float(right_click_match.group(1)),
                "y": float(right_click_match.group(2)),
            }

        # double_click(x, y)
        double_click_match = re.match(
            r"double_click\s*\(\s*([\d.]+)\s*,\s*([\d.]+)\s*\)", action_str
        )
        if double_click_match:
            return {
                "type": "DoubleClickAction",
                "x": float(double_click_match.group(1)),
                "y": float(double_click_match.group(2)),
            }

        # drag(from_x, from_y, to_x, to_y)
        drag_match = re.match(
            r"drag\s*\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*\)", action_str
        )
        if drag_match:
            return {
                "type": "DragAction",
                "from_x": float(drag_match.group(1)),
                "from_y": float(drag_match.group(2)),
                "to_x": float(drag_match.group(3)),
                "to_y": float(drag_match.group(4)),
            }

        # scroll(direction, amount)
        scroll_match = re.match(r"scroll\s*\(\s*(\w+)\s*,\s*(\d+)\s*\)", action_str)
        if scroll_match:
            return {
                "type": "ScrollAction",
                "direction": scroll_match.group(1),
                "amount": int(scroll_match.group(2)),
            }

        # type("text")
        type_match = re.match(r'type\s*\(\s*["\'](.*)["\']\s*\)', action_str)
        if type_match:
            return {
                "type": "TypeAction",
                "text": type_match.group(1),
            }

        # key(key_name)
        key_match = re.match(r"key\s*\(\s*(\w+)\s*\)", action_str)
        if key_match:
            return {
                "type": "KeyAction",
                "key": key_match.group(1),
            }

        # hotkey(key1+key2+...)
        hotkey_match = re.match(r"hotkey\s*\(\s*([\w+]+)\s*\)", action_str)
        if hotkey_match:
            keys = hotkey_match.group(1).split("+")
            return {
                "type": "HotkeyAction",
                "keys": keys,
            }

        # wait(seconds)
        wait_match = re.match(r"wait\s*\(\s*([\d.]+)\s*\)", action_str)
        if wait_match:
            return {
                "type": "WaitAction",
                "seconds": float(wait_match.group(1)),
            }

        # done()
        if re.match(r"done\s*\(\s*\)", action_str):
            return {"type": "DoneAction"}

        # Unknown action, return as done
        return {"type": "DoneAction"}

    def _make_indices(self, ready_envs: List[bool], n: int) -> List[int]:
        """Select n environment indices from ready environments.

        Args:
            ready_envs: Boolean list indicating ready environments
            n: Number of indices to select

        Returns:
            List of selected environment indices
        """
        assert n <= sum(ready_envs), f"Not enough ready envs: {sum(ready_envs)} < {n}"
        ids = np.where(ready_envs)[0]
        np.random.shuffle(ids)
        return ids[:n].tolist()

    def _gather_env_ret(self, first_block: bool = True) -> None:
        """Gather environment returns from the result queue.

        This method collects results from worker processes and:
        - Updates _env_states for ready environments
        - Adds non-init steps to replay buffer
        - Updates running reward on episode completion

        Args:
            first_block: If True, block on first get; always non-blocking after
        """
        if first_block:
            try:
                item = self._env_ret_queue.get(block=True, timeout=60.0)
                self._env_states[item[0]].append(item)
                if not item[1].get("is_init", False):
                    self.replay.add(*item)
                if item[1].get("done", False):
                    self._running_reward = 0.99 * self._running_reward + 0.01 * item[1].get(
                        "reward", 0.0
                    )
            except queue.Empty:
                pass

        # Drain the queue non-blocking
        while True:
            try:
                item = self._env_ret_queue.get(block=False)
                self._env_states[item[0]].append(item)
                if not item[1].get("is_init", False):
                    self.replay.add(*item)
                if item[1].get("done", False):
                    self._running_reward = 0.99 * self._running_reward + 0.01 * item[1].get(
                        "reward", 0.0
                    )
            except queue.Empty:
                break

    def async_step(self, batch_return: Dict[str, Any]) -> None:
        """Dispatch actions to workers based on model output.

        This method:
        1. Decodes response tokens to extract actions
        2. Sends actions to appropriate workers
        3. Triggers environment steps

        Args:
            batch_return: Dict with:
                - prompts: (batch, prompt_len) prompt token IDs
                - responses: (batch, response_len) response token IDs
                - attention_mask: (batch, total_len) combined attention mask
                - worker_id: numpy array of worker IDs
                - meta_info: numpy array of meta info dicts
        """
        actions, meta_infos = self._process_batch_return(batch_return)

        for i in range(len(actions)):
            wid, action = actions[i]
            meta_info = meta_infos[i]
            self._action_queues[wid].put((wid, action, meta_info))
            self._env_global_steps[wid] += 1

    def sample_from_buffer(self, batch_size: Optional[int] = None) -> Dict[str, Any]:
        """Sample a batch from the replay buffer.

        This method waits until enough experiences are available,
        then samples and preprocesses them for training.

        Args:
            batch_size: Number of experiences to sample (default: self.batch_size)

        Returns:
            Batched dict with tokenized experiences including:
            - input_ids: (batch, seq_len) combined prompt + response tokens
            - attention_mask: (batch, seq_len) attention mask
            - responses: (batch, response_len) response tokens only
            - reward: (batch,) reward for each sample
        """
        if batch_size is None:
            batch_size = self.batch_size

        while len(self.replay) < batch_size:
            self._gather_env_ret()

        batch = self.replay.sample(batch_size)
        batch = self._preprocess_batch(batch, include_response=True, use_prev_obs=True)
        batch = collate_fn(batch)
        return batch

    def clear_replay_buffer(self) -> None:
        """Clear all data from the replay buffer."""
        self.replay.clear()

    def get_balance_stats(self) -> Tuple[int, int]:
        """Get balance statistics for replay buffer rewards.

        Returns:
            Tuple of (below_threshold_count, above_threshold_count)
        """
        return self.replay.get_balance_stats()

    def calculate_outcome_reward(self) -> float:
        """Calculate average outcome reward from completed episodes.

        Returns:
            Average reward of completed episodes in replay buffer
        """
        rwds = []
        for item in self.replay.ready_buffer:
            if item is None:
                continue
            wid, data, meta_info = item
            if data.get("done", False):
                rwds.append(data.get("reward", 0.0))
        if len(rwds) > 0:
            return float(np.mean(rwds))
        return 0.0

    def running_outcome_reward(self) -> float:
        """Get the exponentially-smoothed running outcome reward.

        Returns:
            Running average reward (EMA with alpha=0.01)
        """
        return self._running_reward

    def print_examples(self, n: int = 2) -> None:
        """Print n example observations from replay buffer.

        Args:
            n: Number of examples to print
        """
        n = min(n, len(self.replay))
        examples = self.replay.sample(n)
        for e in examples:
            print(e[1].get("obs", ""))

    def print_stats_in_replay_buffer(self) -> None:
        """Print statistics about the replay buffer contents."""
        uid_counts: Dict[str, int] = {}
        for item in self.replay.ready_buffer:
            if item is None:
                continue
            uid = item[2].get("uid", "unknown")
            uid_counts[uid] = uid_counts.get(uid, 0) + 1

        num_uids = len(uid_counts)
        hist: Dict[int, int] = {}
        for count in uid_counts.values():
            hist[count] = hist.get(count, 0) + 1

        stats = (
            f"Replay buffer size: {len(self.replay)}. "
            + f"Number of uids: {num_uids}. "
            + "Histogram of item counts per uid (x-y): "
        )
        counts = []
        for count in sorted(hist.keys()):
            counts.append(f"{count}-{hist[count]}")
        stats = stats + ", ".join(counts)
        print(stats)

    def close(self) -> None:
        """Close all workers and clean up resources."""
        self._running = False

        # Signal workers to stop
        for wid in range(self.num_envs):
            try:
                self._action_queues[wid].put((None, None, None))
            except Exception:
                pass

        # Wait for workers to finish
        for worker in self._workers:
            worker.join(timeout=5)
            if worker.is_alive():
                worker.terminate()
                worker.join(timeout=2)

        # Close queues
        for q in self._action_queues:
            try:
                q.cancel_join_thread()
                q.close()
            except Exception:
                pass

        try:
            self._env_ret_queue.cancel_join_thread()
            self._env_ret_queue.close()
        except Exception:
            pass

    def __del__(self):
        """Destructor - ensure cleanup."""
        try:
            self.close()
        except Exception:
            pass


# Convenience function to create a dataloader from worker URLs
def create_dataloader_from_urls(
    worker_urls: List[str],
    task_configs: List[Dict[str, Any]],
    tokenizer: Any,
    processor: Optional[Any] = None,
    is_multi_modal: bool = False,
    batch_size: int = 8,
    **kwargs,
) -> MultiTurnDataloader:
    """Create a MultiTurnDataloader from worker URLs.

    Args:
        worker_urls: List of worker server URLs
        task_configs: List of task configurations
        tokenizer: HuggingFace tokenizer
        processor: Optional HuggingFace processor for multimodal
        is_multi_modal: Whether to process multimodal inputs
        batch_size: Batch size
        **kwargs: Additional arguments for MultiTurnDataloader

    Returns:
        Configured MultiTurnDataloader instance
    """
    env_configs = [{"server_url": url} for url in worker_urls]

    return MultiTurnDataloader(
        env_class=CBEnvWorkerClient,
        env_configs=env_configs,
        task_configs=task_configs,
        tokenizer=tokenizer,
        processor=processor,
        is_multi_modal=is_multi_modal,
        batch_size=batch_size,
        **kwargs,
    )
