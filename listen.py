import asyncio
import json
import requests
import sounddevice as sd
import numpy as np
import websockets

API_KEY = "8461cb9f-7560-4c62-a120-7173b2696850"
ASSISTANT_ID = "a6210139-4abf-4ed8-b1a5-0996953045b3"
CALL_URL = "https://api.vapi.ai/call"


def create_ws_call():
    headers = {"Authorization": f"Bearer {API_KEY}", "Content-Type": "application/json"}
    payload = {
        "assistantId": ASSISTANT_ID,
        "transport": {
            "provider": "vapi.websocket",
            "audioFormat": {"format": "pcm_s16le", "container": "raw", "sampleRate": 16000},
        },
    }
    resp = requests.post(CALL_URL, headers=headers, json=payload)
    resp.raise_for_status()
    data = resp.json()
    print("✅ Call started:", data)
    ws_url = data["transport"]["websocketCallUrl"]
    print("🔗 Connect to:", ws_url)
    return ws_url


async def audio_producer(queue: asyncio.Queue):
    """Capture mic and push PCM chunks into queue"""

    def callback(indata, frames, time, status):
        if status:
            print("⚠️ Audio status:", status)
        audio_int16 = (indata[:, 0] * 32767).astype(np.int16)
        queue.put_nowait(audio_int16.tobytes())

    with sd.InputStream(samplerate=16000, channels=1, dtype="float32", callback=callback):
        print("🎤 Recording... press Ctrl+C to stop")
        await asyncio.Future()  # run forever


async def audio_consumer(ws, queue: asyncio.Queue):
    """Send audio from queue to websocket"""
    while True:
        chunk = await queue.get()
        await ws.send(chunk)


async def listen(ws):
    """Receive messages (transcripts, control messages, etc.)"""
    async for message in ws:
        try:
            data = json.loads(message)
        except Exception:
            print("🔊 Binary/Non-JSON message received")
            continue

        if data.get("type") == "transcript":
            print("📝 Transcript:", data.get("text"))
        else:
            print("📩 Message:", data)


async def main():
    ws_url = create_ws_call()
    queue = asyncio.Queue()

    async with websockets.connect(ws_url) as ws:
        await asyncio.gather(
            audio_producer(queue),
            audio_consumer(ws, queue),
            listen(ws),
        )


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("👋 Stopped by user")