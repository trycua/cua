import asyncio
import json
import requests
import sounddevice as sd
import numpy as np
import websockets
from hum import record_hum
from midi import convert_to_midi
import time
API_KEY = "8461cb9f-7560-4c62-a120-7173b2696850"
ASSISTANT_ID = "a6210139-4abf-4ed8-b1a5-0996953045b3"
CALL_URL = "https://api.vapi.ai/call"


def create_ws_call():
    headers = {"Authorization": f"Bearer {API_KEY}", "Content-Type": "application/json"}
    payload = {
        "assistantId": ASSISTANT_ID,
        "transport": {
            "provider": "vapi.websocket",
            "audioFormat": {
                "format": "pcm_s16le",
                "container": "raw",
                "sampleRate": 16000,
            },
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
    """Send mic audio from queue to websocket"""
    while True:
        chunk = await queue.get()
        await ws.send(chunk)


async def listen(ws):
    buffer = ""
    phrase_detected = False
    instrument_name = None

    with sd.OutputStream(samplerate=16000, channels=1, dtype="int16") as speaker:
        async for message in ws:
            if isinstance(message, (bytes, bytearray)):
                audio = np.frombuffer(message, dtype=np.int16)
                speaker.write(audio)
                continue

            try:
                data = json.loads(message)
            except Exception:
                continue

            if data.get("type") == "model-output":
                buffer += data.get("output", "")
                print("🧩 Buffer:", buffer)

                if not phrase_detected and "detected, please hum in 5 seconds" in buffer:
                    instrument_name = buffer.split(" detected")[0].strip().split()[-1]
                    print(f"✅ Instrument: {instrument_name}")
                    with open("instrument.txt", "w") as f:
                        f.write(instrument_name)
                    phrase_detected = True

            elif data.get("type") == "speech-update":
                if (
                    phrase_detected
                    and data.get("status") == "stopped"
                    and data.get("role") == "assistant"
                ):
                    print("🔚 Assistant finished phrase, waiting 3s then closing...")
                    
                    await ws.close(code=1000, reason="done")
                    return  # exit listen()


async def main():
    ws_url = create_ws_call()
    queue = asyncio.Queue()

    try:
        async with websockets.connect(ws_url) as ws:
            await asyncio.gather(
                audio_producer(queue),
                audio_consumer(ws, queue),
                listen(ws),
            )
    except Exception as e:
        print("⚠️ Main loop ended:", e)


async def full_pipeline():
    """Complete pipeline: voice assistant -> hum recording -> MIDI -> CUA"""
    # Run websocket loop
    await main()

    # After WS closes, start hum recording
    
    # Get the instrument from the file (set by Vapi assistant)
    instrument = "piano"  # default fallback
    try:
        with open("instrument.txt", "r") as f:
            instrument = f.read().strip()
            print(f"🎵 Detected instrument: {instrument}")
    except FileNotFoundError:
        print("⚠️ No instrument.txt found, using default: piano")
    
    # Create timestamped WAV filename
    timestamp = int(time.time())
    wav_file = f"hum_{timestamp}.wav"
    
    # Record hum and process based on instrument (hum.py handles the routing)
    midi_file = record_hum(wav_file, instrument=instrument)
    
    if midi_file:
        print(f"✅ MIDI file created: {midi_file}")
        
        # Send to CUA for DAW processing
        print("🚀 Sending to CUA for DAW processing...")
        from cua import run_cua
        await run_cua(midi_file, instrument)
    else:
        print("❌ Failed to create MIDI file")

if __name__ == "__main__":
    asyncio.run(full_pipeline())