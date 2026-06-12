import asyncio
import uuid
from dataclasses import asdict, dataclass
from datetime import datetime
from enum import Enum
from typing import Any, Dict, List, Optional

from fastapi import FastAPI, HTTPException
from pydantic import BaseModel


class CompletionStatus(str, Enum):
    PENDING = "pending"
    COMPLETED = "completed"
    FAILED = "failed"


@dataclass
class CompletionCall:
    id: str
    messages: List[Dict[str, Any]]
    model: str
    status: CompletionStatus
    created_at: datetime
    completed_at: Optional[datetime] = None
    response: Optional[str] = None
    tool_calls: Optional[List[Dict[str, Any]]] = None
    error: Optional[str] = None


class ToolCall(BaseModel):
    id: str
    type: str = "function"
    function: Dict[str, Any]


class CompletionRequest(BaseModel):
    messages: List[Dict[str, Any]]
    model: str


class CompletionResponse(BaseModel):
    response: Optional[str] = None
    tool_calls: Optional[List[Dict[str, Any]]] = None


class CompletionQueue:
    def __init__(self):
        self._queue: Dict[str, CompletionCall] = {}
        self._pending_order: List[str] = []
        self._lock = asyncio.Lock()

    async def add_completion(self, messages: List[Dict[str, Any]], model: str) -> str:
        """Add a completion call to the queue."""
        async with self._lock:
            call_id = str(uuid.uuid4())
            completion_call = CompletionCall(
                id=call_id,
                messages=messages,
                model=model,
                status=CompletionStatus.PENDING,
                created_at=datetime.now(),
            )
            self._queue[call_id] = completion_call
            self._pending_order.append(call_id)
            return call_id

    async def get_pending_calls(self) -> List[Dict[str, Any]]:
        """Get all pending completion calls."""
        async with self._lock:
            pending_calls = []
            for call_id in self._pending_order:
                if (
                    call_id in self._queue
                    and self._queue[call_id].status == CompletionStatus.PENDING
                ):
                    call = self._queue[call_id]
                    pending_calls.append(
                        {
                            "id": call.id,
                            "model": call.model,
                            "created_at": call.created_at.isoformat(),
                            "messages": call.messages,
                        }
                    )
            return pending_calls

    async def get_call_status(self, call_id: str) -> Optional[Dict[str, Any]]:
        """Get the status of a specific completion call."""
        async with self._lock:
            if call_id not in self._queue:
                return None

            call = self._queue[call_id]
            result = {
                "id": call.id,
                "status": call.status.value,
                "created_at": call.created_at.isoformat(),
                "model": call.model,
                "messages": call.messages,
            }

            if call.completed_at:
                result["completed_at"] = call.completed_at.isoformat()
            if call.response:
                result["response"] = call.response
            if call.tool_calls:
                result["tool_calls"] = call.tool_calls
            if call.error:
                result["error"] = call.error

            return result

    async def complete_call(
        self,
        call_id: str,
        response: Optional[str] = None,
        tool_calls: Optional[List[Dict[str, Any]]] = None,
    ) -> bool:
        """Mark a completion call as completed with a response or tool calls."""
        async with self._lock:
            if call_id not in self._queue:
                return False

            call = self._queue[call_id]
            if call.status != CompletionStatus.PENDING:
                return False

            call.status = CompletionStatus.COMPLETED
            call.completed_at = datetime.now()
            call.response = response
            call.tool_calls = tool_calls

            # Remove from pending order
            if call_id in self._pending_order:
                self._pending_order.remove(call_id)

            return True

    async def fail_call(self, call_id: str, error: str) -> bool:
        """Mark a completion call as failed with an error."""
        async with self._lock:
            if call_id not in self._queue:
                return False

            call = self._queue[call_id]
            if call.status != CompletionStatus.PENDING:
                return False

            call.status = CompletionStatus.FAILED
            call.completed_at = datetime.now()
            call.error = error

            # Remove from pending order
            if call_id in self._pending_order:
                self._pending_order.remove(call_id)

            return True

    async def wait_for_completion(self, call_id: str, timeout: float = 300.0) -> Optional[str]:
        """Wait for a completion call to be completed and return the response."""
        start_time = asyncio.get_event_loop().time()

        while True:
            status = await self.get_call_status(call_id)
            if not status:
                return None

            if status["status"] == CompletionStatus.COMPLETED.value:
                return status.get("response")
            elif status["status"] == CompletionStatus.FAILED.value:
                raise Exception(f"Completion failed: {status.get('error', 'Unknown error')}")

            # Check timeout
            if asyncio.get_event_loop().time() - start_time > timeout:
                await self.fail_call(call_id, "Timeout waiting for human response")
                raise TimeoutError("Timeout waiting for human response")

            # Wait a bit before checking again
            await asyncio.sleep(0.5)


# Global queue instance
completion_queue = CompletionQueue()

# FastAPI app
app = FastAPI(title="Human Completion Server", version="1.0.0")


@app.post("/queue", response_model=Dict[str, str])
async def queue_completion(request: CompletionRequest):
    """Add a completion request to the queue."""
    call_id = await completion_queue.add_completion(request.messages, request.model)
    return {"id": call_id, "status": "queued"}


@app.get("/pending")
async def list_pending():
    """List all pending completion calls."""
    pending_calls = await completion_queue.get_pending_calls()
    return {"pending_calls": pending_calls}


@app.get("/status/{call_id}")
async def get_status(call_id: str):
    """Get the status of a specific completion call."""
    status = await completion_queue.get_call_status(call_id)
    if not status:
        raise HTTPException(status_code=404, detail="Completion call not found")
    return status


@app.post("/complete/{call_id}")
async def complete_call(call_id: str, response: CompletionResponse):
    """Complete a call with a human response."""
    success = await completion_queue.complete_call(
        call_id, response=response.response, tool_calls=response.tool_calls
    )
    if success:
        return {"status": "success", "message": "Call completed"}
    else:
        raise HTTPException(status_code=404, detail="Call not found or already completed")


@app.post("/fail/{call_id}")
async def fail_call(call_id: str, error: Dict[str, str]):
    """Mark a call as failed."""
    success = await completion_queue.fail_call(call_id, error.get("error", "Unknown error"))
    if not success:
        raise HTTPException(
            status_code=404, detail="Completion call not found or already completed"
        )
    return {"status": "failed"}


@app.get("/")
async def root():
    """Root endpoint."""
    return {"message": "Human Completion Server is running"}


if __name__ == "__main__":
    import uvicorn

    uvicorn.run(app, host="0.0.0.0", port=8002)
