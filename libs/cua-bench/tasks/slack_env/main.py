"""
Slack Clone Environment for CUA-Bench

This environment provides a Slack-like messaging interface for testing
computer use agents on messaging tasks.

Tasks include:
- Sending messages with specific content
- Switching channels
- Replying to messages in threads
- Multi-step messaging workflows
"""

import cua_bench as cb
from pathlib import Path

@cb.tasks_config(split="train")
def load():
    """Generate training tasks for the Slack environment."""
    tasks = []

    # Simple message sending tasks
    messages_to_send = [
        "Hello!",
        "Good morning team!",
        "The deployment is complete.",
        "Can someone review my PR?",
        "Meeting in 5 minutes",
    ]

    for msg in messages_to_send:
        tasks.append(
            cb.Task(
                description=f'Send a message saying "{msg}" in the #general channel.',
                metadata={
                    "task_type": "send_message",
                    "expected_message": msg,
                    "channel": "general",
                },
                computer={
                    "provider": "simulated",
                    "setup_config": {
                        "os_type": "macos",
                        "width": 1280,
                        "height": 800,
                        "background": "#ffcbcb",
                    }
                }
            )
        )

    # Channel switching + messaging tasks
    channels = ["random", "engineering"]
    for channel in channels:
        tasks.append(
            cb.Task(
                description=f'Switch to the #{channel} channel and send "Hello from {channel}!"',
                metadata={
                    "task_type": "channel_message",
                    "expected_message": f"Hello from {channel}!",
                    "channel": channel,
                },
                computer={
                    "provider": "simulated",
                    "setup_config": {
                        "os_type": "macos",
                        "width": 1280,
                        "height": 800,
                        "background": "#ffcbcb",
                    }
                }
            )
        )

    # Thread reply tasks
    thread_tasks = [
        {
            "parent_id": "msg-1",
            "parent_author": "Alice Chen",
            "reply": "Yes, I'm ready!",
        },
        {
            "parent_id": "msg-3",
            "parent_author": "Carol Johnson",
            "reply": "I'll review it now.",
        },
        {
            "parent_id": "msg-2",
            "parent_author": "Bob Smith",
            "reply": "Enjoy your coffee!",
        },
    ]

    for thread_task in thread_tasks:
        tasks.append(
            cb.Task(
                description=f'Reply to {thread_task["parent_author"]}\'s message in a thread with "{thread_task["reply"]}"',
                metadata={
                    "task_type": "thread_reply",
                    "parent_message_id": thread_task["parent_id"],
                    "expected_reply": thread_task["reply"],
                    "channel": "general",
                },
                computer={
                    "provider": "simulated",
                    "setup_config": {
                        "os_type": "macos",
                        "width": 1280,
                        "height": 800,
                        "background": "#ffcbcb",
                    }
                }
            )
        )

    return tasks


@cb.tasks_config(split="test")
def load_test():
    """Generate test tasks for evaluation."""
    return [
        cb.Task(
            description='Send a message containing "benchmark test" to the #general channel.',
            metadata={
                "task_type": "send_message",
                "expected_message": "benchmark test",
                "channel": "general",
            },
            computer={
                "provider": "simulated",
                "setup_config": {
                    "os_type": "macos",
                    "width": 1280,
                    "height": 800,
                    "background": "#ffcbcb",
                }
            }
        ),
        cb.Task(
            description='Switch to #engineering channel and send "deployment ready".',
            metadata={
                "task_type": "channel_message",
                "expected_message": "deployment ready",
                "channel": "engineering",
            },
            computer={
                "provider": "simulated",
                "setup_config": {
                    "os_type": "macos",
                    "width": 1280,
                    "height": 800,
                    "background": "#ffcbcb",
                }
            }
        ),
        cb.Task(
            description='Reply to Carol Johnson\'s message about the PR in a thread saying "LGTM, approved!"',
            metadata={
                "task_type": "thread_reply",
                "parent_message_id": "msg-3",
                "expected_reply": "LGTM, approved!",
                "channel": "general",
            },
            computer={
                "provider": "simulated",
                "setup_config": {
                    "os_type": "macos",
                    "width": 1280,
                    "height": 800,
                    "background": "#ffcbcb",
                }
            }
        ),
    ]


@cb.setup_task(split="train")
async def start(task_cfg, session: cb.DesktopSession | cb.MobileSession):
    """Initialize the Slack environment for a task."""
    # Read the Slack HTML content
    html_content = (Path(__file__).parent / "gui" / "index.html").read_text('utf-8')

    # Launch the Slack window
    await session.launch_window(
        html=html_content,
        title="Slack",
        x=50,
        y=50,
        width=1100,
        height=700,
    )


@cb.setup_task(split="test")
async def start_test(task_cfg, session: cb.DesktopSession | cb.MobileSession):
    """Setup for test split - same as train."""
    await start(task_cfg, session)


@cb.evaluate_task(split="train")
async def evaluate(task_cfg, session: cb.DesktopSession | cb.MobileSession):
    """Evaluate if the task was completed successfully."""
    task_type = task_cfg.metadata.get("task_type", "send_message")

    if task_type == "thread_reply":
        return await evaluate_thread_reply(task_cfg, session)
    else:
        return await evaluate_channel_message(task_cfg, session)


async def evaluate_channel_message(task_cfg, session: cb.DesktopSession | cb.MobileSession):
    """Evaluate channel message tasks."""
    expected_message = task_cfg.metadata.get("expected_message", "")
    expected_channel = task_cfg.metadata.get("channel", "general")
    task_type = task_cfg.metadata.get("task_type", "send_message")

    # Get outbound messages from the Slack API
    outbound_messages = await session.execute_javascript(
        pid=1,
        javascript="window.__slack_shell.getOutboundMessages()"
    )

    if not outbound_messages:
        return [0.0]

    # Check if any message contains the expected text
    for msg in outbound_messages:
        msg_text = msg.get("text", "") if isinstance(msg, dict) else str(msg)
        msg_channel = msg.get("channel", "") if isinstance(msg, dict) else ""

        # Check message content
        if expected_message.lower() in msg_text.lower():
            # For channel-specific tasks, also verify the channel
            if task_type == "channel_message":
                if msg_channel == expected_channel:
                    return [1.0]
            else:
                return [1.0]

    return [0.0]


async def evaluate_thread_reply(task_cfg, session: cb.DesktopSession | cb.MobileSession):
    """Evaluate thread reply tasks."""
    parent_message_id = task_cfg.metadata.get("parent_message_id", "")
    expected_reply = task_cfg.metadata.get("expected_reply", "")

    # Get thread replies from the Slack API
    thread_replies = await session.execute_javascript(
        pid=1,
        javascript="window.__slack_shell.getThreadReplies()"
    )

    if not thread_replies:
        return [0.0]

    # Check if any reply matches the expected content and parent
    for reply in thread_replies:
        reply_text = reply.get("text", "") if isinstance(reply, dict) else str(reply)
        reply_thread_id = reply.get("threadId", "") if isinstance(reply, dict) else ""

        # Check if reply is in the correct thread and contains expected text
        if reply_thread_id == parent_message_id:
            if expected_reply.lower() in reply_text.lower():
                return [1.0]

    return [0.0]


@cb.evaluate_task(split="test")
async def evaluate_test(task_cfg, session: cb.DesktopSession | cb.MobileSession):
    """Evaluation for test split - same as train."""
    return await evaluate(task_cfg, session)


@cb.solve_task(split="train")
async def solve(task_cfg, session: cb.DesktopSession | cb.MobileSession):
    """Automated solution for training tasks."""
    task_type = task_cfg.metadata.get("task_type", "send_message")

    if task_type == "thread_reply":
        await solve_thread_reply(task_cfg, session)
    else:
        await solve_channel_message(task_cfg, session)


async def solve_channel_message(task_cfg, session: cb.DesktopSession | cb.MobileSession):
    """Solve channel message tasks."""
    expected_message = task_cfg.metadata.get("expected_message", "")
    expected_channel = task_cfg.metadata.get("channel", "general")
    task_type = task_cfg.metadata.get("task_type", "send_message")

    # If we need to switch channels first
    if task_type == "channel_message" and expected_channel != "general":
        # Click on the channel in the sidebar
        await session.click_element(1, f'.channel-item[data-channel="{expected_channel}"]')
        if hasattr(session, 'env') and session.env:
            await session.env.step(cb.WaitAction(seconds=0.5))

    # Click on the message input
    await session.click_element(1, "#message-input")
    if hasattr(session, 'env') and session.env:
        await session.env.step(cb.WaitAction(seconds=0.2))

    # Type the message
    if hasattr(session, 'env') and session.env:
        await session.env.step(cb.TypeAction(text=expected_message))
        await session.env.step(cb.WaitAction(seconds=0.2))

    # Click send or press Enter
    if hasattr(session, 'env') and session.env:
        await session.env.step(cb.KeyAction(key="Return"))


async def solve_thread_reply(task_cfg, session: cb.DesktopSession | cb.MobileSession):
    """Solve thread reply tasks."""
    parent_message_id = task_cfg.metadata.get("parent_message_id", "")
    expected_reply = task_cfg.metadata.get("expected_reply", "")

    # Hover over the parent message to show the reply button
    await session.click_element(1, f'[data-message-id="{parent_message_id}"]')
    if hasattr(session, 'env') and session.env:
        await session.env.step(cb.WaitAction(seconds=0.3))

    # Click the reply button to open the thread panel
    await session.click_element(1, f'[data-message-id="{parent_message_id}"] .reply-btn')
    if hasattr(session, 'env') and session.env:
        await session.env.step(cb.WaitAction(seconds=0.5))

    # Click on the thread input
    await session.click_element(1, "#thread-input")
    if hasattr(session, 'env') and session.env:
        await session.env.step(cb.WaitAction(seconds=0.2))

    # Type the reply
    if hasattr(session, 'env') and session.env:
        await session.env.step(cb.TypeAction(text=expected_reply))
        await session.env.step(cb.WaitAction(seconds=0.2))

    # Send the reply
    if hasattr(session, 'env') and session.env:
        await session.env.step(cb.KeyAction(key="Return"))


@cb.solve_task(split="test")
async def solve_test(task_cfg, session: cb.DesktopSession | cb.MobileSession):
    """Solve for test split - same as train."""
    await solve(task_cfg, session)
