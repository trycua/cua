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

    task_configs = [{"env_path": "./task", "task_index": 0, "split": "train"}]
    env_configs = [
        {
            "server_url": f"http://localhost:{8001+i}",
            "task_configs": task_configs,
            "max_step": 50,
            "max_hist": 10,
            "timeout": 300,
        }
        for i in range(4)
    ]

    dataloader = MultiTurnDataloader(
        env_class=CBEnvWorkerClient,
        env_configs=env_configs,
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

import multiprocessing as mp
import random

import numpy as np
import torch
import verl.utils.torch_functional as verl_F
from verl.utils.model import compute_position_id_with_mask


def print_debug(msg):
    print(msg)


def collate_fn(data_list: list[dict]) -> dict:
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


def process_image(image: str):
    import base64
    import io

    from PIL import Image

    jpg_bytes = base64.b64decode(image)
    # Convert bytes to PIL Image
    buffer = io.BytesIO(jpg_bytes)
    image = Image.open(buffer)

    if image.mode != "RGB":
        image = image.convert("RGB")

    return image


def replace_vision_tags(input_string):
    import re

    # List to store the extracted strings
    extracted_strings = []

    # Pattern to match <|vision_start|> followed by any characters until <|vision_end|>
    pattern = r"<\|vision_start\|>(.*?)<\|vision_end\|>"

    # Function to replace matches and collect the inner string
    def replacement(match):
        extracted_strings.append(match.group(1))
        return "<image>"

    # Perform the replacement
    result = re.sub(pattern, replacement, input_string, flags=re.DOTALL)

    return result, extracted_strings


class ReplayBuffer:

    def __init__(self, capacity=10000, gamma=1.0, only_keep_outcome=False, balance_thres=0.1):
        """
        Initialize the replay buffer with improved episode handling

        Args:
            capacity (int): Maximum capacity of the replay buffer
            gamma (float): Discount factor for reward calculation
            only_keep_outcome (bool): Whether to only keep the final step of episodes
        """
        self.capacity = capacity
        self.gamma = gamma
        self.only_keep_outcome = only_keep_outcome
        self.balance_thres = balance_thres

        # Main buffer for ready-to-sample experiences
        self.ready_buffer = [None] * capacity
        self.ready_position = 0
        self.ready_count = 0

        # Temporary storage for ongoing episodes
        self.episode_buffer = {}  # uid -> list of episode steps

    def add(self, data):
        """
        Add data to the replay buffer

        Args:
            data (tuple): A tuple of (worker_id, env_ret, meta_info)
        """
        worker_id, env_ret, meta_info = data
        uid = meta_info["uid"]
        done = env_ret["done"]

        # If this is the first step of an episode, initialize the episode buffer
        if uid not in self.episode_buffer:
            self.episode_buffer[uid] = []

        # Add the current step to the episode buffer
        self.episode_buffer[uid].append(data)

        # If episode is done, process it and add to ready buffer
        if done:
            self._process_episode(uid)

    def _process_episode(self, uid):
        """
        Process a completed episode, calculate discounted rewards,
        and add to the ready buffer

        Args:
            uid (str): Unique ID of the episode
        """
        episode = self.episode_buffer[uid]
        below, above = self.get_balance_stats()

        # Calculate discounted returns for each step (G = r + gamma * G_next)
        # First, get the immediate rewards for each step
        rewards = [step[1]["reward"] for step in episode]

        # Calculate discounted returns (working backwards)
        discounted_returns = [0] * len(rewards)
        discounted_returns[-1] = rewards[-1]  # Last step return is just its reward

        # Calculate returns for all previous steps
        for i in range(len(rewards) - 2, -1, -1):
            discounted_returns[i] = rewards[i] + self.gamma * discounted_returns[i + 1]

        # If only keeping outcome, just store the final step
        if self.only_keep_outcome:
            # Only add the final step
            self._add_to_ready_buffer(episode[-1])
        else:
            # Add all steps with properly calculated returns
            for i in range(len(episode)):
                # Copy the data to avoid modifying the original
                worker_id, env_ret, meta_info = episode[i]
                env_ret = env_ret.copy()  # Shallow copy is sufficient

                # Set the discounted return
                env_ret["reward"] = discounted_returns[i]

                # Add to ready buffer
                self._add_to_ready_buffer((worker_id, env_ret, meta_info))

        # Clear the episode from temporary storage
        del self.episode_buffer[uid]

    def _add_to_ready_buffer(self, data):
        """
        Add a processed experience to the ready buffer

        Args:
            data (tuple): Processed experience data
        """
        # Replace the oldest item if buffer is full
        self.ready_buffer[self.ready_position] = data

        # Update position
        self.ready_position = (self.ready_position + 1) % self.capacity

        # Update count
        self.ready_count = min(self.ready_count + 1, self.capacity)

    def get_balance_stats(self):
        thres = self.balance_thres
        below = 0
        above = 0
        for data in self.ready_buffer:
            if data is None:
                continue
            _, env_ret, _ = data
            env_ret = env_ret["reward"]
            if env_ret < thres:
                below += 1
            else:
                above += 1
        return below, above

    def should_keep(self, curr_below, curr_above, curr_ret):
        # if rate -> 1, then there are too many high return states
        # if rate -> -1, there are are too many low return states
        # we need to balance it to around 0
        thres = self.balance_thres
        rate = (curr_above - curr_below) / (curr_above + curr_below + 1e-8)
        keep = False
        if -0.1 < rate < 0.1:
            keep = True
        elif rate >= 0.1:
            keep = True if curr_ret < thres else False
        else:
            keep = True if curr_ret > thres else False
        # add noise to revert the decision
        # keep = not keep if np.random.uniform() < 0.1 else keep
        return keep

    def sample(self, batch_size):
        """
        Sample experiences from the ready buffer

        Args:
            batch_size (int): Number of experiences to sample

        Returns:
            list: List of sampled experiences
        """
        if self.ready_count == 0:
            return []

        valid_indices = [i for i in range(self.ready_count) if self.ready_buffer[i] is not None]

        if len(valid_indices) < batch_size:
            sampled_indices = random.sample(valid_indices, len(valid_indices))
        else:
            sampled_indices = random.sample(valid_indices, batch_size)

        return [self.ready_buffer[i] for i in sampled_indices]

    def clear(self):
        """
        Clear both ready buffer and episode buffer
        """
        self.ready_buffer = [None] * self.capacity
        self.ready_position = 0
        self.ready_count = 0
        self.episode_buffer = {}

    def __len__(self):
        """
        Return the number of valid experiences in the ready buffer
        """
        return sum(1 for item in self.ready_buffer if item is not None)


def env_worker_process(
    env_config,
    recv_queue,
    send_queue,
    worker_id,
    env_class,
):
    """Worker process that runs environment rollouts.

    Args:
        env_config: Configuration dict for the environment client.
            Must contain 'task_configs' key with list of task configurations.
        recv_queue: Queue to receive actions from main process
        send_queue: Queue to send results to main process
        worker_id: ID of this worker
        env_class: Environment client class (e.g., CBEnvWorkerClient)
    """
    # Create environment client with env_config dict
    env = env_class(env_config)

    # Initial reset to random task_config
    env_ret, meta_info = env.reset()
    send_queue.put((worker_id, env_ret, meta_info))

    while True:
        item = recv_queue.get()
        wid, action, _ = item
        if wid is None and action is None:
            break
        assert wid == worker_id
        env_ret, meta_info = env.step(action)
        send_queue.put((worker_id, env_ret, meta_info))
        if env_ret["done"]:
            item = recv_queue.get()
            env_ret, meta_info = env.reset()
            send_queue.put((worker_id, env_ret, meta_info))


class MultiTurnDataloader:
    """Dataloader for RL training with parallel environment workers.

    Each env_config must contain a 'task_configs' key with a list of task
    configurations that the client will use internally.
    """

    def __init__(
        self,
        env_class,
        env_configs,
        tokenizer,
        processor=None,  # for multi-modal
        is_multi_modal=True,
        batch_size=8,
        replay_capacity=10000,
        replay_reward_discount=0.9,
        max_prompt_length=1024,
        max_response_length=1024,
        only_keep_outcome_in_replay=False,
    ):
        self.num_envs = len(env_configs)
        self._tokenizer = tokenizer
        self._processor = processor
        self._is_multi_modal = is_multi_modal
        self.batch_size = batch_size
        assert (
            self.batch_size <= self.num_envs
        ), "Each env cannot run more than one step in a data batch."
        self.replay = ReplayBuffer(
            replay_capacity, replay_reward_discount, only_keep_outcome_in_replay
        )
        self._only_keep_outcome_in_replay = only_keep_outcome_in_replay

        self._action_queues = [mp.Queue() for _ in range(self.num_envs)]
        self._env_ret_queue = mp.Queue()

        self._env_states = [[] for _ in range(self.num_envs)]
        self._env_global_steps = [0 for _ in range(self.num_envs)]
        self._max_prompt_length = max_prompt_length
        self._max_response_length = max_response_length
        self._running_reward = 0

        self._workers = []
        for wid in range(self.num_envs):
            w = mp.Process(
                target=env_worker_process,
                args=(
                    env_configs[wid],
                    self._action_queues[wid],
                    self._env_ret_queue,
                    wid,
                    env_class,
                ),
            )
            w.daemon = True
            w.start()
            self._workers.append(w)

        self._running = True

    def __iter__(self):
        return self

    def __next__(self):
        self._gather_env_ret(first_block=False)
        ready_envs = [len(self._env_states[i]) > 0 for i in range(self.num_envs)]
        print_debug(f"[DataLoader] Number of ready envs: {sum(ready_envs)}")
        while sum(ready_envs) < self.batch_size:
            print_debug("[DataLoader] No ready envs, waiting for envs to return data...")
            self._gather_env_ret()
            ready_envs = [len(self._env_states[i]) > 0 for i in range(self.num_envs)]
            print_debug(f"[DataLoader] Number of ready envs: {sum(ready_envs)}")
        ids = self._make_indices(ready_envs, self.batch_size)
        batch = [self._env_states[i].pop(0) for i in ids]
        batch = self._preprocess_batch(batch, include_response=False, use_prev_obs=False)
        batch = collate_fn(batch)
        return batch

    def _preprocess_batch(self, batch, include_response=True, use_prev_obs=True):
        rows = []
        for wid, state, meta_info in batch:
            row_dict = {}
            prompt = state["prev_obs"] if use_prev_obs else state["obs"]

            if self._is_multi_modal:
                prompt, jpg_strs = replace_vision_tags(prompt)
                raw_prompt = prompt.replace(
                    "<image>", "<|vision_start|><|image_pad|><|vision_end|>"
                )
                row_dict["multi_modal_data"] = {
                    "image": [process_image(image) for image in jpg_strs]
                }
                image_inputs = self._processor.image_processor(
                    row_dict["multi_modal_data"]["image"], return_tensors="pt"
                )
                image_grid_thw = image_inputs["image_grid_thw"]
                row_dict["multi_modal_inputs"] = {key: val for key, val in image_inputs.items()}

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
            else:
                raw_prompt = prompt

            input_ids, attention_mask = verl_F.tokenize_and_postprocess_data(
                prompt=prompt,
                tokenizer=self._tokenizer,
                max_length=self._max_prompt_length,
                pad_token_id=self._tokenizer.pad_token_id,
                left_pad=True,
                truncation="left",
            )

            if include_response:
                response_ids, response_attention_mask = verl_F.tokenize_and_postprocess_data(
                    prompt=state["action"],
                    tokenizer=self._tokenizer,
                    max_length=self._max_response_length,
                    pad_token_id=self._tokenizer.pad_token_id,
                    left_pad=False,
                    truncation="right",
                )
                input_ids = torch.cat([input_ids, response_ids], dim=1)
                attention_mask = torch.cat([attention_mask, response_attention_mask], dim=1)
                row_dict["responses"] = response_ids[0]
                row_dict["reward"] = torch.tensor(state["reward"]).float()

            if self._is_multi_modal:
                from verl.models.transformers.qwen2_vl import get_rope_index

                position_ids = get_rope_index(
                    self._processor,
                    input_ids=input_ids[0],
                    image_grid_thw=image_grid_thw,
                    attention_mask=attention_mask[0],
                )  # (3, seq_len)
            else:
                position_ids = compute_position_id_with_mask(attention_mask)

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

    def _process_batch_return(self, batch_return):
        actions = []
        meta_infos = []
        for i in range(len(batch_return["prompts"])):
            # prompt_ids = batch_return["prompts"][i]
            prompt_length = batch_return["prompts"][i].shape[-1]

            # valid_prompt_length = batch_return["attention_mask"][i][:prompt_length].sum()
            # valid_prompt_ids = prompt_ids[-valid_prompt_length:]

            response_ids = batch_return["responses"][i]
            valid_response_length = batch_return["attention_mask"][i][prompt_length:].sum()
            valid_response_ids = response_ids[:valid_response_length]
            sequences_str = self._tokenizer.decode(valid_response_ids)
            actions.append((batch_return["worker_id"][i], sequences_str))
            meta_infos.append(batch_return["meta_info"][i])

        return actions, meta_infos

    def _make_indices_sync(self, n):
        assert n <= self.num_envs
        s_min = min(self._env_global_steps)
        s_max = max(self._env_global_steps)
        assert s_max - s_min <= 1, "All envs should progress together"

        arr = np.array(self._env_global_steps)
        list_smin = np.where(arr == s_min)[0]
        list_smax = np.where(arr == s_max)[0]

        np.random.shuffle(list_smin)
        np.random.shuffle(list_smax)

        if n <= len(list_smin):
            indices = list_smin[:n]
        else:
            needed_from_smax = n - len(list_smin)
            indices = np.concatenate((list_smin, list_smax[:needed_from_smax]))

        return indices

    def _make_indices(self, ready_envs, n):
        assert n <= sum(ready_envs)
        ids = np.where(ready_envs)[0]
        np.random.shuffle(ids)
        indices = ids[:n]
        return indices

    def _gather_env_ret(self, first_block=True):
        if first_block:
            item = self._env_ret_queue.get(block=True)
            self._env_states[item[0]].append(item)
            if not item[1]["is_init"]:
                self.replay.add(item)
            if item[1]["done"]:
                self._running_reward = 0.99 * self._running_reward + 0.01 * item[1]["reward"]
        while True:
            try:
                item = self._env_ret_queue.get(block=False)
                self._env_states[item[0]].append(item)
                if not item[1]["is_init"]:
                    self.replay.add(item)
                if item[1]["done"]:
                    self._running_reward = 0.99 * self._running_reward + 0.01 * item[1]["reward"]
            except Exception:
                break

    def async_step(self, batch_return):
        actions, meta_infos = self._process_batch_return(batch_return)
        for i in range(len(actions)):
            wid, action = actions[i]
            meta_info = meta_infos[i]
            self._action_queues[wid].put((wid, action, meta_info))
            self._env_global_steps[wid] += 1

    def sample_from_buffer(self, batch_size=None):
        if batch_size is None:
            batch_size = self.batch_size
        while len(self.replay) < batch_size:
            self._gather_env_ret()

        batch = self.replay.sample(batch_size)
        batch = self._preprocess_batch(batch, include_response=True, use_prev_obs=True)
        batch = collate_fn(batch)
        return batch

    def clear_replay_buffer(self):
        self.replay.clear()

    def get_balance_stats(self):
        below, above = self.replay.get_balance_stats()
        return below, above

    def calculate_outcome_reward(self):
        rwds = []
        for item in self.replay.ready_buffer:
            if item is None:
                continue
            wid, data, meta_info = item
            if data["done"]:
                rwds.append(data["reward"])
        if len(rwds) > 0:
            return np.mean(rwds)
        return 0

    def print_examples(self, n=2):
        n = min(n, len(self.replay))
        examples = self.replay.sample(n)
        for e in examples:
            print(e[1]["obs"])

    def print_stats_in_replay_buffer(self):
        uid_counts = {}
        for item in self.replay.ready_buffer:
            if item is None:
                continue
            uid = item[2]["uid"]  # meta_info is the third element, access 'uid'
            if uid in uid_counts:
                uid_counts[uid] += 1
            else:
                uid_counts[uid] = 1
        num_uids = len(uid_counts)
        hist = {}
        for count in uid_counts.values():
            if count in hist:
                hist[count] += 1
            else:
                hist[count] = 1

        stats = (
            f"Replay buffer size: {len(self.replay)}. "
            + f"Number of uids: {num_uids}. "
            + "Histogram of item counts per uid in replay buffer (x-y): "
        )
        counts = []
        for count in sorted(hist.keys()):
            counts.append(f"{count}-{hist[count]}")
        stats = stats + ", ".join(counts)
        print(stats)

    def running_outcome_reward(self):
        return self._running_reward

    def close(self):
        """Close all workers and clean up resources."""
        self._running = False

        # Signal workers to stop - send twice because worker may be waiting
        # at either the outer loop or inner loop (after episode done)
        for wid in range(self.num_envs):
            try:
                self._action_queues[wid].put((None, None, None))
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
