"""
GUI-Owl Agent implementation using HuggingFace models.

This agent integrates the GUI-Owl model (Mobile-Agent-V3) into the CUA
agent framework, enabling local multimodal GUI automation with
click, scroll, drag, type, and keyboard actions.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import TYPE_CHECKING, Any, Dict, List, Optional

from . import register_agent
from .base import AgentResult, BaseAgent, FailureMode

if TYPE_CHECKING:
    from ..computers import DesktopSession


@register_agent("gui-owl")
class GUIOwlAgent(BaseAgent):
    """
    GUI-Owl Agent.

    Uses a HuggingFace-hosted GUI-Owl model to reason over screenshots
    and emit structured GUI actions that are executed via DesktopSession.
    """

    def __init__(self, **kwargs):
        super().__init__(**kwargs)
        self.model_name: str = kwargs.get("model", "mPLUG/GUI-Owl-7B")
        self.device: str = kwargs.get("device", "auto")
        self.max_steps: int = kwargs.get("max_steps", 50)

        self._model = None
        self._processor = None

    @staticmethod
    def name() -> str:
        return "gui-owl"

    # ---------------------------------------------------------------------
    # Model loading
    # ---------------------------------------------------------------------
    def _lazy_load_model(self):
        """
        Lazily load the GUI-Owl model and processor.

        This avoids introducing a hard dependency unless the agent
        is explicitly used.
        """
        if self._model is not None and self._processor is not None:
            return self._model, self._processor

        try:
            import torch
            from transformers import AutoModelForCausalLM, AutoProcessor
        except Exception as e:
            raise RuntimeError(
                "GUI-Owl requires transformers and torch. "
                "Install with: pip install transformers torch"
            ) from e

        dtype = torch.float16 if torch.cuda.is_available() else torch.float32

        self._processor = AutoProcessor.from_pretrained(self.model_name)
        self._model = AutoModelForCausalLM.from_pretrained(
            self.model_name,
            torch_dtype=dtype,
            device_map=self.device,
        )

        self._model.eval()
        return self._model, self._processor

    # ---------------------------------------------------------------------
    # Output parsing
    # ---------------------------------------------------------------------
    def _parse_model_output(self, text: str) -> Optional[Dict[str, Any]]:
        """
        Parse model output into a structured action dict.

        Expected format (example):
        {
          "action": "click",
          "x": 320,
          "y": 210
        }
        """
        try:
            return json.loads(text)
        except Exception:
            return None

    # ---------------------------------------------------------------------
    # Action mapping
    # ---------------------------------------------------------------------
    async def _map_action_to_execution(
        self,
        action_dict: Dict[str, Any],
        session: "DesktopSession",
    ) -> Optional[Any]:
        """
        Convert parsed action dict into cua_bench Action
        and execute it.
        """
        from ..types import (
            ClickAction,
            DoubleClickAction,
            RightClickAction,
            MiddleClickAction,
            DragAction,
            ScrollAction,
            TypeAction,
            KeyAction,
            HotkeyAction,
            MoveToAction,
            WaitAction,
            DoneAction,
        )

        action_type = action_dict.get("action")
        action = None

        # Click actions
        if action_type == "click":
            action = ClickAction(x=int(action_dict["x"]), y=int(action_dict["y"]))
        elif action_type == "double_click":
            action = DoubleClickAction(x=int(action_dict["x"]), y=int(action_dict["y"]))
        elif action_type == "right_click":
            action = RightClickAction(x=int(action_dict["x"]), y=int(action_dict["y"]))
        elif action_type == "middle_click":
            action = MiddleClickAction(x=int(action_dict["x"]), y=int(action_dict["y"]))

        # Drag
        elif action_type == "drag":
            action = DragAction(
                from_x=int(action_dict["from_x"]),
                from_y=int(action_dict["from_y"]),
                to_x=int(action_dict["to_x"]),
                to_y=int(action_dict["to_y"]),
                duration=float(action_dict.get("duration", 1.0)),
            )

        # Scroll
        elif action_type == "scroll":
            action = ScrollAction(
                direction=action_dict.get("direction", "down"),
                amount=int(action_dict.get("amount", 100)),
            )

        # Mouse move
        elif action_type == "move_to":
            action = MoveToAction(
                x=int(action_dict["x"]),
                y=int(action_dict["y"]),
                duration=float(action_dict.get("duration", 0.0)),
            )

        # Typing / keys
        elif action_type == "type":
            action = TypeAction(text=action_dict.get("text", ""))
        elif action_type == "key":
            action = KeyAction(key=action_dict.get("key"))
        elif action_type == "hotkey":
            action = HotkeyAction(keys=action_dict.get("keys", []))

        # Control
        elif action_type == "wait":
            action = WaitAction(seconds=float(action_dict.get("seconds", 1.0)))
        elif action_type == "done":
            action = DoneAction()

        if action is None:
            return None

        await session.execute_action(action)
        return action

    # ---------------------------------------------------------------------
    # Main agent loop
    # ---------------------------------------------------------------------
    async def perform_task(
        self,
        task_description: str,
        session: "DesktopSession",
        logging_dir: Path | None = None,
        tracer=None,
    ) -> AgentResult:
        """
        Execute a task using GUI-Owl.

        The agent repeatedly:
        - captures a screenshot
        - queries the model
        - parses an action
        - executes it
        """
        model, processor = self._lazy_load_model()

        instruction = self._render_instruction(task_description)
        total_usage = {"prompt_tokens": 0, "completion_tokens": 0}

        step = 0
        task_completed = False

        try:
            while step < self.max_steps and not task_completed:
                step += 1
                print(f"[GUI-Owl Step {step}]")

                screenshot_bytes = await session.screenshot()

                inputs = processor(
                    images=screenshot_bytes,
                    text=instruction,
                    return_tensors="pt",
                )

                outputs = model.generate(
                    **inputs,
                    max_new_tokens=256,
                )

                decoded = processor.batch_decode(outputs, skip_special_tokens=True)[0]
                print(f"[Model Output]: {decoded}")

                action_dict = self._parse_model_output(decoded)

                if not action_dict:
                    print("[WARN] Could not parse model output")
                    break

                if action_dict.get("action") == "done":
                    task_completed = True
                    break

                action = await self._map_action_to_execution(action_dict, session)

                if action is None:
                    print("[WARN] Unknown action emitted")
                    break

                if tracer:
                    try:
                        tracer.record(
                            "agent_action",
                            {
                                "step": step,
                                "agent": self.name(),
                                "model": self.model_name,
                                "action": action_dict,
                            },
                            [screenshot_bytes],
                        )
                    except Exception:
                        pass

            failure_mode = (
                FailureMode.NONE
                if task_completed
                else FailureMode.MAX_STEPS_EXCEEDED
            )

            return AgentResult(
                total_input_tokens=total_usage["prompt_tokens"],
                total_output_tokens=total_usage["completion_tokens"],
                failure_mode=failure_mode,
            )

        except Exception as e:
            print(f"[ERROR] GUI-Owl agent failed: {e}")
            return AgentResult(
                total_input_tokens=0,
                total_output_tokens=0,
                failure_mode=FailureMode.UNKNOWN,
            )
