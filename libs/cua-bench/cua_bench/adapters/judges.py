"""LLM judges for the live-web benchmarks (need ``OPENAI_API_KEY``).

Each judge keeps its upstream prompts verbatim and its upstream verdict rule:

* :func:`webvoyager` from MinorJerry/WebVoyager ``evaluation/auto_eval.py``
  (Apache-2.0, Copyright the WebVoyager authors), commit 5a78967.
* :func:`webjudge` from OSU-NLP-Group/Online-Mind2Web
  ``src/methods/webjudge_online_mind2web.py`` (MIT, Copyright (c) 2025 OSU
  Natural Language Processing), commit f0d805e.
* :func:`webgym` from microsoft/webgym ``webgym/models/base/evaluation_prompt.py``
  (MIT, Copyright (c) Microsoft Corporation), commit 23028f0: every
  reference fact verified by the screenshots (criterion A) and, when the
  agent answered, the answer supported by them (criterion B).

The judged evidence is what cua-bench has at evaluate time: the final
screenshot(s), the agent's answer (``session.final_answer``) and action
history (``session.action_history``). ``chat`` is any async
``messages -> text`` callable; :class:`OpenAIChat` is the default
(``OPENAI_API_KEY``, optional ``OPENAI_BASE_URL``; model from
``CUA_BENCH_JUDGE_MODEL`` or the judge's default).
"""

from __future__ import annotations

import asyncio
import base64
import os
import re
from typing import Any, Awaitable, Callable, Optional, Sequence

Chat = Callable[[list], Awaitable[str]]


class OpenAIChat:
    """Chat completions over HTTP (no SDK dependency)."""

    def __init__(self, model: str, *, api_key: Optional[str] = None,
                 base_url: Optional[str] = None, timeout: float = 180.0, **params: Any) -> None:
        self.model = os.environ.get("CUA_BENCH_JUDGE_MODEL") or model
        self.api_key = api_key or os.environ.get("OPENAI_API_KEY")
        if not self.api_key:
            raise RuntimeError("the judge needs OPENAI_API_KEY")
        self.base_url = (base_url or os.environ.get("OPENAI_BASE_URL") or "https://api.openai.com/v1").rstrip("/")
        self.timeout = timeout
        self.params = params

    async def __call__(self, messages: list) -> str:
        import httpx

        body = {"model": self.model, "messages": messages, **self.params}
        async with httpx.AsyncClient(timeout=self.timeout) as client:
            for attempt in range(4):  # bounded retries on rate limits / 5xx
                r = await client.post(f"{self.base_url}/chat/completions", json=body,
                                      headers={"Authorization": f"Bearer {self.api_key}"})
                if r.status_code in (429, 500, 502, 503) and attempt < 3:
                    await asyncio.sleep(5 * (attempt + 1))
                    continue
                r.raise_for_status()
                return r.json()["choices"][0]["message"]["content"] or ""
        raise RuntimeError("judge: no answer")


def _png_url(png: bytes) -> dict:
    return {"type": "image_url",
            "image_url": {"url": "data:image/png;base64," + base64.b64encode(png).decode()}}


# ── WebVoyager ────────────────────────────────────────────────────────────

WEBVOYAGER_SYSTEM = """As an evaluator, you will be presented with three primary components to assist you in your role:

1. Web Task Instruction: This is a clear and specific directive provided in natural language, detailing the online activity to be carried out. These requirements may include conducting searches, verifying information, comparing prices, checking availability, or any other action relevant to the specified web service (such as Amazon, Apple, ArXiv, BBC News, Booking etc).

2. Result Screenshots: This is a visual representation of the screen showing the result or intermediate state of performing a web task. It serves as visual proof of the actions taken in response to the instruction.

3. Result Response: This is a textual response obtained after the execution of the web task. It serves as textual result in response to the instruction.

-- You DO NOT NEED to interact with web pages or perform actions such as booking flights or conducting searches on websites.
-- You SHOULD NOT make assumptions based on information not presented in the screenshot when comparing it to the instructions.
-- Your primary responsibility is to conduct a thorough assessment of the web task instruction against the outcome depicted in the screenshot and in the response, evaluating whether the actions taken align with the given instructions.
-- NOTE that the instruction may involve more than one task, for example, locating the garage and summarizing the review. Failing to complete either task, such as not providing a summary, should be considered unsuccessful.
-- NOTE that the screenshot is authentic, but the response provided by LLM is generated at the end of web browsing, and there may be discrepancies between the text and the screenshots.
-- Note the difference: 1) Result response may contradict the screenshot, then the content of the screenshot prevails, 2) The content in the Result response is not mentioned on the screenshot, choose to believe the content.

You should elaborate on how you arrived at your final evaluation and then provide a definitive verdict on whether the task has been successfully accomplished, either as 'SUCCESS' or 'NOT SUCCESS'."""

WEBVOYAGER_USER = """TASK: <task>
Result Response: <answer>
<num> screenshots at the end: """


async def webvoyager(task: str, answer: str, screenshots: Sequence[bytes], chat: Chat) -> Optional[float]:
    """1.0 SUCCESS, 0.0 NOT SUCCESS, None when the verdict is missing (upstream rule)."""
    user = (WEBVOYAGER_USER.replace("<task>", task).replace("<answer>", answer)
            .replace("<num>", str(len(screenshots))))
    messages = [
        {"role": "system", "content": WEBVOYAGER_SYSTEM},
        {"role": "user", "content": [{"type": "text", "text": user}]
         + [_png_url(s) for s in screenshots] + [{"type": "text", "text": "Your verdict:\n"}]},
    ]
    text = await chat(messages)
    if "NOT SUCCESS" in text:
        return 0.0
    return 1.0 if "SUCCESS" in text else None


# ── Online-Mind2Web WebJudge ──────────────────────────────────────────────

WEBJUDGE_KEYPOINTS_SYSTEM = """You are an expert tasked with analyzing a given task to identify the key points explicitly stated in the task description.

**Objective**: Carefully analyze the task description and extract the critical elements explicitly mentioned in the task for achieving its goal.

**Instructions**:
1. Read the task description carefully.
2. Identify and extract **key points** directly stated in the task description.
   - A **key point** is a critical element, condition, or step explicitly mentioned in the task description.
   - Do not infer or add any unstated elements.
   - Words such as "best," "highest," "cheapest," "latest," "most recent," "lowest," "closest," "highest-rated," "largest," and "newest" must go through the sort function(e.g., the key point should be "Filter by highest").

**Respond with**:
- **Key Points**: A numbered list of the explicit key points for completing this task, one per line, without explanations or additional details."""

WEBJUDGE_IMAGE_SYSTEM = (  # verbatim, including upstream's trailing spaces
    'You are an expert evaluator tasked with determining whether an image contains information about the necessary steps to complete a task.\n'
    '\n'
    '**Objective**: Analyze the provided image and decide if it shows essential steps or evidence required for completing the task. Use your reasoning to explain your decision before assigning a score.\n'
    '\n'
    '**Instructions**:\n'
    '1. Provide a detailed description of the image, including its contents, visible elements, text (if any), and any notable features.\n'
    '\n'
    '2. Carefully examine the image and evaluate whether it contains necessary steps or evidence crucial to task completion:  \n'
    '- Identify key points that could be relevant to task completion, such as actions, progress indicators, tool usage, applied filters, or step-by-step instructions.  \n'
    '- Does the image show actions, progress indicators, or critical information directly related to completing the task?  \n'
    '- Is this information indispensable for understanding or ensuring task success?\n'
    '- If the image contains partial but relevant information, consider its usefulness rather than dismissing it outright.\n'
    '\n'
    '3. Provide your response in the following format:  \n'
    '- **Reasoning**: Explain your thought process and observations. Mention specific elements in the image that indicate necessary steps, evidence, or lack thereof.  \n'
    '- **Score**: Assign a score based on the reasoning, using the following scale:  \n'
    '    - **1**: The image does not contain any necessary steps or relevant information.  \n'
    '    - **2**: The image contains minimal or ambiguous information, unlikely to be essential.  \n'
    '    - **3**: The image includes some relevant steps or hints but lacks clarity or completeness.  \n'
    '    - **4**: The image contains important steps or evidence that are highly relevant but not fully comprehensive.  \n'
    '    - **5**: The image clearly displays necessary steps or evidence crucial for completing the task.\n'
    '\n'
    'Respond with:  \n'
    '1. **Reasoning**: [Your explanation]  \n'
    '2. **Score**: [1-5]'
)

WEBJUDGE_IMAGE_USER = """**Task**: {task}

**Key Points for Task Completion**: {key_points}

The snapshot of the web page is shown in the image."""

WEBJUDGE_SYSTEM = """You are an expert in evaluating the performance of a web navigation agent. The agent is designed to help a human user navigate a website to complete a task. Given the user's task, the agent's action history, key points for task completion, some potentially important web pages in the agent's trajectory and their reasons, your goal is to determine whether the agent has completed the task and achieved all requirements.

Your response must strictly follow the following evaluation criteria!
*Important Evaluation Criteria*:
1: The filtered results must be displayed correctly. If filters were not properly applied (i.e., missing selection, missing confirmation, or no visible effect in results), the task is not considered successful.
2: You must carefully check whether these snapshots and action history meet these key points. Ensure that specific filter conditions, such as "best," "highest," "cheapest," "latest," "most recent," "lowest," "closest," "highest-rated," "largest," and "newest" are correctly applied using the filter function(e.g., sort function).
3: Certain key points or requirements should be applied by the filter. Otherwise, a search with all requirements as input will be deemed a failure since it cannot guarantee that all results meet the requirements!
4: If the task requires filtering by a specific range of money, years, or the number of beds and bathrooms, the applied filter must exactly match the given requirement. Any deviation results in failure. To ensure the task is successful, the applied filter must precisely match the specified range without being too broad or too narrow.
Examples of Failure Cases:
- If the requirement is less than $50, but the applied filter is less than $25, it is a failure.
- If the requirement is $1500-$2500, but the applied filter is $2000-$2500, it is a failure.
- If the requirement is $25-$200, but the applied filter is $0-$200, it is a failure.
- If the required years are 2004-2012, but the filter applied is 2001-2012, it is a failure.
- If the required years are before 2015, but the applied filter is 2000-2014, it is a failure.
- If the task requires exactly 2 beds, but the filter applied is 2+ beds, it is a failure.
5: Some tasks require a submission action or a display of results to be considered successful.
6: If the retrieved information is invalid or empty(e.g., No match was found), but the agent has correctly performed the required action, it should still be considered successful.
7: If the current page already displays all available items, then applying a filter is not necessary. As long as the agent selects items that meet the requirements (e.g., the cheapest or lowest price), the task is still considered successful.

*IMPORTANT*
Format your response into two lines as shown below:

Thoughts: <your thoughts and reasoning process based on double-checking each key points and the evaluation criteria>
Status: "success" or "failure"
"""

WEBJUDGE_USER = """User Task: {task}

Key Points: {key_points}

Action History:
{last_actions}

The potentially important snapshots of the webpage in the agent's trajectory and their reasons:
{thoughts}"""

WEBJUDGE_USER_NO_IMAGES = """User Task: {task}

Key Points: {key_points}

Action History:
{last_actions}"""


async def webjudge(task: str, actions: Sequence[str], screenshots: Sequence[bytes], chat: Chat,
                   score_threshold: int = 3) -> float:
    """WebJudge: key points, per-snapshot relevance scores, then the final status."""
    kp = await chat([{"role": "system", "content": WEBJUDGE_KEYPOINTS_SYSTEM},
                     {"role": "user", "content": [{"type": "text", "text": f"Task: {task}"}]}])
    kp = kp.replace("\n\n", "\n")
    kp = kp.split("**Key Points**:")[1] if "**Key Points**:" in kp else kp.split("Key Points:")[-1]
    key_points = "\n".join(line.lstrip() for line in kp.splitlines())

    async def judge_image(png: bytes) -> str:
        return await chat([
            {"role": "system", "content": WEBJUDGE_IMAGE_SYSTEM},
            {"role": "user", "content": [
                {"type": "text", "text": WEBJUDGE_IMAGE_USER.format(task=task, key_points=key_points)},
                _png_url(png)]},
        ])

    responses = await asyncio.gather(*(judge_image(s) for s in screenshots))
    images, thoughts = [], []
    for response, png in zip(responses, screenshots):
        try:
            score = int(re.findall(r"[1-5]", response.split("Score")[1])[0])
            thought = response.split("**Reasoning**:")[-1].strip().lstrip("\n").split("\n\n")[0].replace("\n", " ")
        except (IndexError, ValueError):
            score, thought = 0, ""
        if score >= score_threshold:
            images.append(_png_url(png))
            if thought:
                thoughts.append(thought)
    history = "\n".join(f"{i + 1}. {a}" for i, a in enumerate(actions))
    template = WEBJUDGE_USER if images else WEBJUDGE_USER_NO_IMAGES
    text = template.format(task=task, key_points=key_points, last_actions=history,
                           thoughts="\n".join(f"{i + 1}. {t}" for i, t in enumerate(thoughts)))
    verdict = await chat([{"role": "system", "content": WEBJUDGE_SYSTEM},
                          {"role": "user", "content": [{"type": "text", "text": text}] + images}])
    status = verdict.lower().split("status:")[-1]
    return 1.0 if "success" in status and "failure" not in status else 0.0


# ── WebGym ────────────────────────────────────────────────────────────────

WEBGYM_A_SYSTEM = """You are an expert in evaluating the performance of a web navigation agent. The agent is designed to help a human user navigate a website to complete a task. Your goal is to verify whether a SPECIFIC FACT can be confirmed by the provided screenshots.

As an evaluator, you will be presented with the following components:

1. Task Instruction: The original task description (provided for CONTEXT ONLY)
2. Fact Group: A group of related facts decomposed from the task instruction (provided for CONTEXT ONLY)
3. Fact to Check: A specific fact that you need to verify (THIS IS YOUR PRIMARY FOCUS)
4. Trajectory: A complete list of observations and actions that were taken by the agent
5. Result Screenshots: Visual representation of the screen showing the result or intermediate state

CRITICAL: Your judgment should ONLY focus on whether the FACT TO CHECK can be verified by the screenshots. You are NOT checking the agent's response - only whether the screenshots contain evidence for the fact.

Guidelines for evaluation:
-- Your primary responsibility is to assess whether the screenshots contain evidence that verifies the FACT TO CHECK.
-- The fact to check may involve more than one sub-fact. ALL sub-facts must be verifiable from the screenshots.
-- If the fact requires specific information (e.g., "concert is in the US or Canada"), the screenshots must show this information.
-- If the evaluation criteria asks to find a specific item, the screenshots must show that exact item (not a similar one).

IMPORTANT - Handling "OR" conditions:
-- When the fact or task contains "OR" (e.g., "best books on cooking OR gardening OR home decor"), satisfying ANY ONE of the alternatives is sufficient for SUCCESS.
-- Example: If the task is "find best books on cooking OR gardening OR home decor" and the screenshots show best cooking books, this is SUCCESS - the agent does NOT need to find all three.
-- "OR" indicates alternatives/options, not a requirement to verify all items.

Response format (you should STRICTLY follow the format):
1. Analysis: [Describe what evidence you see in the screenshots related to the fact to check]
2. Verdict: [SUCCESS if the fact is verified by screenshots, NOT SUCCESS otherwise]
"""

WEBGYM_A_USER = """
===Your Turn===
Task Instruction (for context only):
[task_instruction]

Fact Group (for context only):
[fact_group]

Fact to Check (PRIMARY FOCUS - verify this against the screenshots):
[fact_to_check]

Completion history:
[trajectory]

Relevant screenshots:
attached.

Evaluation: (MUST end with line "2. Verdict: [SUCCESS or NOT SUCCESS]")"""

WEBGYM_B_SYSTEM = """You are an expert in detecting hallucinations in web navigation agent responses. Your goal is to verify whether the agent's FINAL RESPONSE is supported by the provided screenshots.

As an evaluator, you will be presented with the following components:

1. Task Instruction: The task the agent was trying to complete
2. Final Response: The agent's answer/response to the task
3. Result Screenshots: Visual representation of the screens the agent visited

CRITICAL: Your job is to check if the agent's response contains information that is NOT shown in the screenshots. Agents frequently hallucinate or make up answers that are not verified by what they actually saw.

Guidelines for evaluation:
-- Check whether EVERY claim in the agent's response can be verified by the screenshots.
-- If the response mentions specific facts (names, numbers, dates, locations, etc.), these MUST be visible in the screenshots.
-- If the response contains information not shown in ANY screenshot, this is a hallucination - mark as NOT SUCCESS.
-- If the response is vague or says "I couldn't find the information", check if this matches what the screenshots show.
-- YOU SHOULD EXPECT THAT THERE IS A HIGH CHANCE THAT THE AGENT WILL MAKE UP AN ANSWER NOT VERIFIED BY THE SCREENSHOTS.

Response format (you should STRICTLY follow the format):
1. Claims in response: [List the specific claims/facts in the agent's response]
2. Screenshot verification: [For each claim, state whether it appears in the screenshots]
3. Verdict: [SUCCESS if ALL claims are verified by screenshots, NOT SUCCESS if ANY claim is not verified]
"""

WEBGYM_B_USER = """
===Your Turn===
Task Instruction:
[task_instruction]

Agent's Final Response:
[response]

Screenshots from the agent's session:
attached.

Evaluation: (MUST end with line "3. Verdict: [SUCCESS or NOT SUCCESS]")"""


def _verdict(text: str) -> bool:
    tail = text.split("Verdict")[-1] if "Verdict" in text else text
    return "SUCCESS" in tail and "NOT SUCCESS" not in tail


async def webgym(task: str, reference: Sequence[dict], answer: str, actions: Sequence[str],
                 screenshots: Sequence[bytes], chat: Chat) -> float:
    """1.0 when every reference fact is verified (and the answer, if any, is supported)."""
    trajectory = "\n".join(actions) or "(not recorded)"
    images = [_png_url(s) for s in screenshots]
    checks = []
    for group in reference:
        facts = list(group.get("facts") or []) or [group.get("description", "")]
        for fact in facts:
            user = (WEBGYM_A_USER.replace("[task_instruction]", task)
                    .replace("[fact_group]", str(group.get("description", "")))
                    .replace("[fact_to_check]", str(fact)).replace("[trajectory]", trajectory))
            checks.append(chat([{"role": "system", "content": WEBGYM_A_SYSTEM},
                                {"role": "user", "content": [{"type": "text", "text": user}] + images}]))
    if answer:
        user = WEBGYM_B_USER.replace("[task_instruction]", task).replace("[response]", answer)
        checks.append(chat([{"role": "system", "content": WEBGYM_B_SYSTEM},
                            {"role": "user", "content": [{"type": "text", "text": user}] + images}]))
    verdicts = await asyncio.gather(*checks)
    return 1.0 if verdicts and all(_verdict(v) for v in verdicts) else 0.0
